use anyhow::{ensure, Context, Result};
use async_openai::types::{
    ChatCompletionRequestSystemMessageArgs, ChatCompletionRequestUserMessageArgs,
    ChatCompletionResponseMessage, CreateChatCompletionRequestArgs,
};
use dirs::home_dir;
use inquire::{required, CustomType, Password, PasswordDisplayMode, Text};
use regex::Regex;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use spinoff::{spinners, Spinner};
use std::{path::Path, time::Duration};
use tokio::{
    fs::{create_dir_all, read_to_string, write},
    process::Command,
};
use which::which;

#[derive(Serialize, Deserialize)]
struct Config {
    api_base_url: String,  // The base URL of the Inference API provider
    api_key: String,       // Your API key from the Inference API provider
    model_name: String,    // The ID of the model to use
    system_prompt: String, // The contents of the system prompt
    user_prompt: String,   // The contents of the user prompt
    max_tokens: u16,       // The maximum number of tokens that can be generated
    request_timeout: u64,  // The timeout for the request in seconds
}

#[derive(Deserialize)]
struct CommitMessageCandidate {
    message: ChatCompletionResponseMessage, // This stores a single commit message candidate generated by the model
}

#[derive(Deserialize)]
struct CommitMessageCandidates {
    choices: Vec<CommitMessageCandidate>, // This stores all the commit message candidates generated by the model
}

async fn load_config_file(config_file: &Path) -> Result<Config> {
    let config = if config_file.exists() {
        // Read config file if exists
        toml::from_str(
            &read_to_string(config_file)
                .await
                .context("Failed to read the configuration file")?,
        )
        .context("Failed to parse the configuration file")?
    } else {
        // Ask for API base URL
        let api_base_url = Text::new("Enter API base URL:")
            .with_default("https://api.together.xyz/v1")
            .prompt()?;

        // Ask for API key
        let api_key = Password::new("Enter your API key:")
            .with_display_toggle_enabled()
            .with_display_mode(PasswordDisplayMode::Masked)
            .with_validator(required!("API key is required"))
            .without_confirmation()
            .prompt()?;

        // Ask for model name
        let model_name = Text::new("Enter model name:")
            .with_default("mistralai/Mixtral-8x7B-Instruct-v0.1")
            .with_validator(required!("Model name is required"))
            .prompt()?;

        // Ask for system prompt
        let system_prompt = Text::new("Enter system prompt:")
            .with_default("You are required to write a meaningful commit message for the given code changes. The commit message must have the format: `type(scope): description`. The `type` must be one of the following: feat, fix, docs, style, refactor, perf, test, build, ci, chore, or revert. The `scope` indicates the area of the codebase that the changes affect. The `description` must be concise and written in a single sentence without a period at the end.")
            .with_validator(required!("System prompt is required"))
            .with_help_message("Press Enter to use the default system prompt")
            .prompt()?;

        // Ask for user prompt
        let user_prompt = Text::new("Enter user prompt:")
            .with_default("The output of the git diff command:\n```\n{}\n```")
            .with_validator(required!("User prompt is required"))
            .with_help_message("Press Enter to use the default user prompt")
            .prompt()?;

        // Ask for max tokens
        let max_tokens = CustomType::<u16>::new("Enter max tokens of generated commit messages:")
            .with_default(128)
            .with_help_message("Press Enter to use the default max tokens")
            .prompt()?;

        // Ask for request timeout
        let request_timeout = CustomType::<u64>::new("Enter request timeout (in seconds):")
            .with_default(30)
            .with_help_message("Press Enter to use the default request timeout")
            .prompt()?;

        // Create a config instance with the provided values
        let config = Config {
            api_base_url: api_base_url.trim().to_string(),
            api_key: api_key.trim().to_string(),
            model_name: model_name.trim().to_string(),
            system_prompt: system_prompt.trim().to_string(),
            user_prompt: user_prompt.trim().to_string(),
            max_tokens,
            request_timeout,
        };

        // Create config directory if not exists
        create_dir_all(
            config_file
                .parent()
                .context("Failed to retrieve the configuration directory")?,
        )
        .await
        .context("Failed to create config directory")?;

        // Write config to file
        write(
            config_file,
            toml::to_string(&config).context("Failed to serialize the configuration")?,
        )
        .await
        .context("Failed to write config to file")?;

        println!("Config file created successfully: {:?}", config_file);

        config
    };

    Ok(config)
}

async fn run_git_command(args: &[&str]) -> Result<String> {
    // Run Git command with the given arguments
    let res = Command::new("git")
        .args(args)
        .output()
        .await
        .context("Failed to execute the Git command")?;

    // If the command failed, return early with the error from stderr
    ensure!(
        res.status.success(),
        "{}",
        String::from_utf8_lossy(&res.stderr) // It's fine if stderr has invalid UTF-8 characters
    );

    // Return the command output only if stdout has no invalid UTF-8 characters
    Ok(String::from_utf8(res.stdout).context("Failed to decode the output of the Git command")?)
}

async fn generate_commit_message(
    http_client: &Client,
    config: &Config,
    git_diffs: &str,
) -> Result<String> {
    let payload = CreateChatCompletionRequestArgs::default()
        .max_tokens(config.max_tokens)
        .model(&config.model_name)
        .messages([
            ChatCompletionRequestSystemMessageArgs::default()
                .content(&config.system_prompt)
                .build()?
                .into(),
            ChatCompletionRequestUserMessageArgs::default()
                .content(config.user_prompt.replace("{}", git_diffs))
                .build()?
                .into(),
        ])
        .build()
        .context("Failed to construct the request payload")?;

    let response = http_client
        .post(format!("{}/chat/completions", &config.api_base_url))
        .bearer_auth(&config.api_key)
        .json(&payload)
        .send()
        .await
        .context("Failed to send the request to the Inference API provider")?
        .error_for_status()?
        .json::<CommitMessageCandidates>()
        .await
        .context("Failed to parse the response from the Inference API provider")?;

    let commit_message = response
        .choices
        .first() // Only the first generated commit message is used
        .context("No commit messages generated")?
        .message
        .content
        .as_ref()
        .context("No commit messages generated")?;

    // Post-process the generated commit message to keep only the first line and remove leading and trailing backticks
    let regex_matches = Regex::new(r"(?m)^\s*(?:`\s*(.+?)\s*`|(.+?))\s*$")?
        .captures(&commit_message)
        .context("Failed to post-process the generated commit message")?;

    let commit_message = regex_matches
        .get(1)
        .or(regex_matches.get(2))
        .context("Failed to post-process the generated commit message")?
        .as_str()
        .to_string();

    Ok(commit_message)
}

#[tokio::main]
async fn main() -> Result<()> {
    // Check if Git is installed
    which("git").context(
        "Git not found, please install it first or check your PATH environment variable",
    )?;

    // Check if the current directory is a Git repository
    run_git_command(&["rev-parse", "--is-inside-work-tree"])
        .await
        .context("The current directory is not a Git repository")?;

    // Get staged diffs
    let git_diffs = run_git_command(&[
        "--no-pager",
        "diff",
        "--staged",
        "--minimal",
        "--no-color",
        "--no-ext-diff",
        "--",
        ":!*.lock", // Ignore .lock files
    ])
    .await?
    .trim()
    .to_string();

    // Verify there are staged changes
    ensure!(!git_diffs.is_empty(), "No staged changes to commit");

    // Path to config file
    let config_file = home_dir()
        .context("Failed to retrieve the user's home directory")?
        .join(".acm/config.toml");

    // Load config file or create if not exists
    let config = load_config_file(&config_file).await?;

    // Create an HTTP client to interact with the Inference API
    let http_client = Client::builder()
        .timeout(Duration::from_secs(config.request_timeout))
        .build()?;

    // Start spinner
    let mut spinner = Spinner::new(spinners::Dots, "Generating commit message", None);

    // Generate commit message using a LLM
    let commit_message = generate_commit_message(&http_client, &config, &git_diffs).await;

    // Stop the spinner
    spinner.stop_with_message("");

    let commit_message = commit_message?;

    // Ask user to edit the generated commit message if needed
    let edited_commit_message = Text::new("Your generated commit message:")
        .with_initial_value(&commit_message)
        .with_validator(required!(
            "Please provide a commit message to create a commit"
        ))
        .with_help_message(
            "Press Enter to create a new commit with the current message or ESC to cancel",
        )
        .prompt()?;

    // Commit the changes with the commit message and print the output of the `git commit -m <message>` command
    println!(
        "{}",
        &run_git_command(&["commit", "-m", edited_commit_message.trim()]).await?
    );

    Ok(())
}

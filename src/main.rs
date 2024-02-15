use anyhow::{ensure, Context, Result};
use inquire::{required, CustomType, Password, PasswordDisplayMode, Text};
use serde::{Deserialize, Serialize};
use spinoff::{spinners, Spinner};
use std::process::{Command, Stdio};
use std::{path::Path, time::Duration};
use ureq::{json, serde_json};
use which::which;

#[derive(Default, Serialize, Deserialize)]
struct Config {
    api_base_url: String,  // The base URL of the Inference API provider
    api_key: String,       // Your API key from the Inference API provider
    model_name: String,    // The ID of the model to use
    system_prompt: String, // The contents of the system prompt
    user_prompt: String,   // The contents of the user prompt
    max_tokens: u16,       // The maximum number of tokens that can be generated
    request_timeout: u64,  // The timeout for the request in seconds
}

fn load_config_file(config_file: &Path) -> Result<Config> {
    let config = if config_file.exists() {
        // Read config file if exists
        confy::load_path(config_file).context("Failed to read the configuration file")?
    } else {
        // Ask for API base URL
        let api_base_url = Text::new("Enter API base URL:")
            .with_default("https://api.together.xyz/v1")
            .with_help_message("Press Enter to use the default API base URL")
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
            .with_help_message("Press Enter to use the default model name")
            .prompt()?;

        // Ask for system prompt
        let system_prompt = Text::new("Enter system prompt:")
            .with_default("You are required to write a meaningful commit message for the given code changes. The commit message must have the format: `type(scope): description`. The `type` must be one of the following: feat, fix, docs, style, refactor, perf, test, build, ci, chore, or revert. The `scope` indicates the area of the codebase that the changes affect. The `description` must be concise and written in a single sentence without a period at the end.")
            .with_help_message("Press Enter to use the default system prompt")
            .prompt()?;

        // Ask for user prompt
        let user_prompt = Text::new("Enter user prompt:")
            .with_default("The output of the git diff command:\n```\n{}\n```")
            .with_help_message("Press Enter to use the default user prompt")
            .prompt()?;

        // Ask for max tokens
        let max_tokens = CustomType::<u16>::new("Enter max tokens of generated commit messages:")
            .with_default(64)
            .with_help_message("Press Enter to use the default max tokens")
            .prompt()?;

        // Ask for request timeout
        let request_timeout = CustomType::<u64>::new("Enter request timeout (in seconds):")
            .with_default(10)
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

        // Write config to file
        confy::store_path(config_file, &config)
            .context("Failed to write the configuration file")?;

        println!("Config file created successfully: {:?}", config_file);

        config
    };

    Ok(config)
}

fn run_git_command(args: &[&str]) -> Result<String> {
    // Run Git command with the given arguments
    let command = Command::new("git")
        .args(args)
        .stderr(Stdio::inherit())
        .output()
        .context("Failed to execute the Git command")?;

    // Return an error message if an error occurred while executing the Git command
    ensure!(
        command.status.success(),
        "An error occurred while executing the Git command"
    );

    // Return the command output only if stdout has no invalid UTF-8 characters
    String::from_utf8(command.stdout).context("Failed to decode the output of the Git command")
}

fn generate_commit_message(config: &Config, git_diffs: &str) -> Result<String> {
    let response = ureq::post(&format!("{}/chat/completions", &config.api_base_url))
        .timeout(Duration::from_secs(config.request_timeout))
        .set("Authorization", &format!("Bearer {}", &config.api_key))
        .send_json(json!({
            "model": config.model_name,
            "n": 1,
            "max_tokens": config.max_tokens,
            "messages": [
                { "role": "system", "content": config.system_prompt },
                { "role": "user", "content": config.user_prompt.replace("{}", git_diffs) }
            ]
        }))
        .context("Failed to send the request to the Inference API provider")?
        .into_json::<serde_json::Value>()
        .context("Failed to parse the response from the Inference API provider")?;

    let commit_message = response["choices"][0]["message"]["content"]
        .as_str()
        .context("No commit messages generated")?
        .to_string();

    Ok(commit_message)
}

fn main() -> Result<()> {
    // Add -V/--version and -h/--help flags to the CLI
    clap::command!().get_matches();

    // Check if Git is installed
    which("git").context(
        "Git not found, please install it first or check your PATH environment variable",
    )?;

    // Check if the current directory is a Git repository
    run_git_command(&["rev-parse", "--is-inside-work-tree"])
        .context("The current directory is not a Git repository")?;

    // Get staged diffs
    let git_diffs = run_git_command(&[
        "--no-pager",
        "diff",
        "--staged",
        "--minimal",
        "--no-color",
        "--function-context",
        "--no-ext-diff",
        "--",
        ":!*.lock",  // Ignore .lock files
        ":!*.lockb", // Ignore .lockb files
    ])?
    .trim()
    .to_string();

    // Verify there are staged changes
    ensure!(!git_diffs.is_empty(), "No staged changes to commit");

    // Path to config file
    let config_file = dirs::home_dir()
        .context("Failed to retrieve the user's home directory")?
        .join(".config/acm/config.toml");

    // Load config file or create if not exists
    let config = load_config_file(&config_file)?;

    // Start spinner
    let mut spinner = Spinner::new(spinners::Dots, "Generating commit message", None);

    // Generate commit message using a LLM
    let commit_message = generate_commit_message(&config, &git_diffs)?;

    // Stop the spinner
    spinner.stop_with_message("");

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
        &run_git_command(&["commit", "-m", edited_commit_message.trim()])?
    );

    Ok(())
}

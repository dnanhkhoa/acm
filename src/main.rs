use anyhow::{ensure, Context, Result};
use inquire::{required, Text};
use serde::{Deserialize, Serialize};
use spinoff::{spinners::Dots, Spinner};
use std::process::{Command, Stdio};
use ureq::{json, serde_json, serde_json::Value};
use which::which;

#[derive(Serialize, Deserialize)]
struct Config {
    base_url: String,               // Base URL of API endpoints
    api_key: String,                // Your API key
    params: Value, // Parameters of the model being used (e.g. https://docs.together.ai/reference/chat-completions)
    custom_message: Option<String>, // Custom commit message when using JSON mode
}

impl Default for Config {
    fn default() -> Self {
        Self {
            base_url: "https://api.perplexity.ai".to_string(),
            api_key: String::new(),
            params: json!({
                "model": "llama-3-70b-instruct",
                "max_tokens": 256,
                "n": 1,
                "messages": [
                    {
                        "role": "system",
                        "content": "\
                        You will be provided with the output from the `git diff --staged` command.\n\
                        Your task is to craft a concise and descriptive commit message that accurately reflects the code changes.\n\
                        \n\
                        Please adhere to the Conventional Commits specification, formatting the message as follows:\n\
                        <type>(<scope>): <description>\n\
                        \n\
                        - `type`: Choose one of the following based on the nature of the changes:\n\
                        * feat: A new feature\n\
                        * fix: A bug fix\n\
                        * docs: Documentation changes\n\
                        * style: Changes that do not affect the meaning of the code (formatting, whitespace, etc.)\n\
                        * refactor: A code change that neither fixes a bug nor adds a feature\n\
                        * perf: A code change that improves performance\n\
                        * test: Adding missing tests or correcting existing tests\n\
                        * build: Changes that affect the build system or external dependencies\n\
                        * ci: Changes to the CI configuration files and scripts\n\
                        * chore: Other changes that don't modify src or test files\n\
                        \n\
                        - `scope` (optional): A specific area or module of the codebase that the changes affect, enclosed in parentheses (e.g., `feat(parser):`)\n\
                        - `description`: A concise summary of the changes in a single, lowercase sentence without ending punctuation\n\
                        \n\
                        Please provide only the commit message in your response, as it will be used directly in a git commit command.\
                        "
                    }
                ]
            }),
            custom_message: None,
        }
    }
}

fn run_git_command(args: &[&str]) -> Result<String> {
    let command = Command::new("git")
        .args(args)
        .stderr(Stdio::inherit())
        .output()
        .context("Failed to execute the git command")?;

    ensure!(
        command.status.success(),
        "The git command exited with an error"
    );

    String::from_utf8(command.stdout).context("The git command returned invalid UTF-8")
}

fn generate_commit_message(config: &mut Config, git_diffs: &str) -> Result<String> {
    config.params["messages"]
        .as_array_mut()
        .context("Missing `messages` parameter in the config file")?
        .push(json!({
            "role": "user",
            "content": git_diffs
        }));

    let response = ureq::post(&format!("{}/chat/completions", &config.base_url))
        .set("Authorization", &format!("Bearer {}", &config.api_key))
        .send_json(&config.params)?
        .into_json::<Value>()?;

    ensure!(
        response["choices"][0]["finish_reason"]
            .as_str()
            .ne(&Some("length")),
        "The generated message exceeded `max_tokens`"
    );

    let message = response["choices"][0]["message"]["content"]
        .as_str()
        .unwrap_or_default();

    if config.params["response_format"]["type"]
        .as_str()
        .eq(&Some("json_object"))
    {
        if let Some(custom_message) = &config.custom_message {
            let json_message = serde_json::from_str::<Value>(message)?;

            return Ok(custom_message
                .split("||")
                .map(|chunk| {
                    json_message
                        .pointer(chunk)
                        .and_then(|value| value.as_str())
                        .unwrap_or(chunk)
                })
                .collect::<String>());
        }
    }

    Ok(message.to_string())
}

fn main() -> Result<()> {
    clap::command!().get_matches(); // Add the `--version` flag to the CLI

    which("git").context("Unable to find git executable in PATH")?;

    run_git_command(&["rev-parse", "--is-inside-work-tree"])
        .context("The current directory is not a git repository")?;

    let git_diffs = run_git_command(&[
        "--no-pager",
        "diff",
        "--staged",
        "--minimal",
        "--no-color",
        "--function-context",
        "--no-ext-diff",
        "--",
        ":(exclude)*.lock*", // Ignore files ending with .lock and any extension after
        ":(exclude)*-lock.*", // Ignore files with -lock. in the name
    ])?
    .trim()
    .to_string();

    ensure!(!git_diffs.is_empty(), "No changes staged for commit");

    let config_file = dirs::home_dir()
        .context("Failed to get the home directory")?
        .join(".config/acm/config.toml");

    let mut config = confy::load_path::<Config>(&config_file)?;

    ensure!(
        !config.api_key.is_empty(),
        "Please provide your API key in the config file created at {:?}",
        config_file
    );

    let mut spinner = Spinner::new(Dots, "Generating a commit message", None);

    let commit_message = generate_commit_message(&mut config, &git_diffs);

    spinner.stop_with_message("");

    let edited_commit_message = Text::new("Message:")
        .with_initial_value(&commit_message?)
        .with_validator(required!("A message is required to create a commit"))
        .with_help_message("Press Enter to create a new commit or ESC to cancel")
        .prompt()?;

    println!(
        "{}",
        &run_git_command(&["commit", "-m", edited_commit_message.trim()])?
    );

    Ok(())
}

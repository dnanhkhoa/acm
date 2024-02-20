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
            base_url: "https://api.together.xyz/v1".to_string(),
            api_key: String::new(),
            params: json!({
                "model": "mistralai/Mixtral-8x7B-Instruct-v0.1",
                "max_tokens": 128,
                "temperature": 0,
                "top_p": 0.1,
                "n": 1,
                "messages": [
                    {
                        "role": "system",
                        "content": "\
                        You will be provided with an output from the `git diff --staged` command. Your task is to construct a clean and comprehensive commit message for the code changes in JSON format with the following keys:\n\
                        - type: A label from the following list [feat, fix, docs, style, refactor, perf, test, build, ci, chore] that represents the code changes\n\
                        - description: A succinct description of the code changes in a single sentence, without a period at the end\
                        "
                    }
                ],
                "response_format": {
                    "type": "json_object",
                    "schema": {
                        "type": "object",
                        "properties": {
                            "type": {
                                "type": "string"
                            },
                            "description": {
                                "type": "string"
                            }
                        },
                        "required": [
                            "type",
                            "description"
                        ]
                    }
                }
            }),
            custom_message: Some("||/type||: ||/description||".to_string()),
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
        ":!*.lock",  // Ignore .lock files
        ":!*.lockb", // Ignore .lockb files
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

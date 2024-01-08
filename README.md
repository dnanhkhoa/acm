# AI Commit Message `acm`

A dead-simple AI-powered CLI tool for effortlessly crafting meaningful Git commit messages.

![Demo](assets/demo.gif)

## Features

- Create meaningful commit messages with ease
- Support for [Conventional Commits standard](https://www.conventionalcommits.org)
- Customizable prompts
- Compatible with various LLM API providers, including [OpenAI](https://openai.com), [OpenRouter](http://openrouter.ai), [Together AI](https://www.together.ai), [Anyscale](https://www.anyscale.com), and more.

## Installation

Before installing `acm`, please make sure you have [git](https://git-scm.com) installed on your system.

### Cargo

To install `acm` globally using `Cargo`, run the following command:

```sh
cargo install --locked acm-cli
```

When you run the tool for the first time, it will prompt you for some information, including the `API Base URL`, `API Key`, and `Model Name`. You can leave the other fields as default.

## Usage

To generate a commit message and commit your changes, simply use `acm` as a replacement for `git commit`:

```sh
# Stage your changes
git add <files...>

# Generate a commit message and commit your changes
acm
```

## License

`acm` is licensed under the [Apache License 2.0](https://choosealicense.com/licenses/apache-2.0/)

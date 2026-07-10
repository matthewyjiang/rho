use std::io::{self, Read};

use crate::{
    agent::Agent,
    cli::Command,
    herdr::{HerdrReporter, HerdrState},
};

pub(super) fn prompt_for_command(command: &Option<Command>) -> anyhow::Result<Option<String>> {
    match command {
        Some(Command::Run { prompt, stdin }) => prompt_from_stdin(prompt.clone(), *stdin).map(Some),
        Some(Command::Login { .. }) | Some(Command::Update) | None => Ok(None),
    }
}

pub(super) async fn run(
    agent: &mut Agent,
    prompt: String,
    herdr: &HerdrReporter,
) -> anyhow::Result<()> {
    herdr.report_state(HerdrState::Working, None, None).await;
    let result = agent.run(prompt).await;
    herdr.report_state(HerdrState::Idle, None, None).await;
    herdr.release().await;
    let answer = result?;
    println!("{answer}");
    Ok(())
}

fn prompt_from_stdin(parts: Vec<String>, read_stdin: bool) -> anyhow::Result<String> {
    prompt_from_reader(parts, read_stdin, &mut io::stdin())
}

fn prompt_from_reader(
    parts: Vec<String>,
    read_stdin: bool,
    stdin: &mut impl Read,
) -> anyhow::Result<String> {
    let mut chunks = Vec::new();
    let inline = parts.join(" ").trim().to_string();
    if !inline.is_empty() {
        chunks.push(inline);
    }
    if read_stdin {
        let mut buffer = String::new();
        stdin.read_to_string(&mut buffer)?;
        let buffer = buffer.trim().to_string();
        if !buffer.is_empty() {
            chunks.push(buffer);
        }
    }

    let prompt = chunks.join("\n\n");
    if prompt.is_empty() {
        anyhow::bail!("rho run requires a prompt argument or --stdin");
    }
    Ok(prompt)
}

#[cfg(test)]
#[path = "automation_tests.rs"]
mod tests;

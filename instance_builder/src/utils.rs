pub async fn exec_string_command(command: &str) -> anyhow::Result<()> {
    let parts = shell_words::split(command)?;
    let mut cmd = tokio::process::Command::new(&parts[0]);
    if parts.len() > 1 {
        cmd.args(&parts[1..]);
    }
    let status = cmd.status().await?;
    if !status.success() {
        return Err(anyhow::anyhow!("Command failed"));
    }
    Ok(())
}

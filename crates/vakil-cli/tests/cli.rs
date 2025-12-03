use assert_cmd::prelude::*;
use predicates::prelude::*;
use std::process::Command;

#[test]
fn smoke_help() -> Result<(), Box<dyn std::error::Error>> {
    let mut cmd = Command::cargo_bin("vakil-cli")?;
    cmd.arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("USAGE").or(predicate::str::contains("vakil-cli")));
    Ok(())
}

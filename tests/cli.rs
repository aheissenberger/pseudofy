use assert_cmd::Command;
use predicates::prelude::*;
use predicates::str::contains;

fn pseudofy() -> Command {
    Command::cargo_bin("pseudofy").unwrap()
}

#[test]
fn version_prints_name_and_version() {
    pseudofy()
        .arg("--version")
        .assert()
        .success()
        .stdout("pseudofy 0.1.0\n");
}

#[test]
fn help_lists_subcommands() {
    pseudofy()
        .arg("--help")
        .assert()
        .success()
        .stdout(contains("init").and(contains("run")));
}

#[test]
fn subcommands_are_not_implemented() {
    for subcommand in ["init", "run"] {
        pseudofy()
            .arg(subcommand)
            .assert()
            .code(2)
            .stderr(contains("not implemented"));
    }
}

#[test]
fn unknown_subcommand_fails() {
    pseudofy().arg("unknown").assert().failure();
}

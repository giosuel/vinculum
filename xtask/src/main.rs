//! `cargo xtask <task>`: repository tooling.
//!
//! - `dep-age [--days N]`: every crate version in every committed `Cargo.lock` must be public
//!   for at least 14 days (README.md, "Dependency versions").

use std::path::Path;
use std::process::ExitCode;

mod dep_age;

const FAILED: u8 = 1;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.split_first() {
        Some((task, rest)) if task == "dep-age" => dep_age_task(rest),
        _ => {
            eprintln!("usage: cargo xtask dep-age [--days N]");
            ExitCode::from(2)
        }
    }
}

/// `cargo xtask dep-age [--days N]`
fn dep_age_task(args: &[String]) -> ExitCode {
    let days = match args {
        [] => 14,
        [flag, n] if flag == "--days" => match n.parse() {
            Ok(n) => n,
            Err(_) => {
                eprintln!("error: --days needs a number");
                return ExitCode::from(2);
            }
        },
        _ => {
            eprintln!("usage: cargo xtask dep-age [--days N]");
            return ExitCode::from(2);
        }
    };
    // xtask/ sits directly in the workspace root.
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().expect("xtask is inside the workspace");
    let report = match dep_age::check(root, days) {
        Ok(report) => report,
        Err(err) => {
            eprintln!("dep-age: couldn't run: {err}");
            return ExitCode::from(2);
        }
    };
    for (exception, why) in &report.unneeded {
        eprintln!(
            "dep-age: note: the exception for {} {} is no longer needed ({why}): remove it from xtask/dep-age-exceptions.txt",
            exception.name, exception.version
        );
    }
    if report.findings.is_empty() {
        println!(
            "dep-age: ok ({} crate versions in {} lockfiles, all at least {days} days old)",
            report.packages, report.files
        );
        return ExitCode::SUCCESS;
    }
    eprintln!("dep-age: {} crate version(s) younger than {days} days:", report.findings.len());
    for finding in &report.findings {
        let package = &finding.package;
        let fix = match &finding.suggestion {
            Some(version) => format!("cargo update -p {}@{} --precise {version}", package.name, package.version),
            None => "no older compatible version; wait, or add an exception with a reason".into(),
        };
        eprintln!(
            "  - {} {} (published {}, {} days ago): {fix}",
            package.name, package.version, finding.published, finding.age_days
        );
    }
    eprintln!("Security fixes and owner exceptions may be listed in xtask/dep-age-exceptions.txt with a reason.");
    ExitCode::from(FAILED)
}

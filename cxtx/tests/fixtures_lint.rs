//! P4.2 fixture redaction lint — fails the build if any committed file
//! under `cxtx/tests/fixtures/` matches a secret / PII regex.

use std::fs;
use std::path::{Path, PathBuf};

const SECRET_PATTERN: &str = r"(?i)(sk|OPENAI|ANTHROPIC).{0,5}[_-]?(KEY|TOKEN)";
const EMAIL_PATTERN: &str = r"@strongdm\.";

fn fixtures_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(dir).unwrap_or_else(|e| panic!("read_dir {}: {e}", dir.display())) {
        let entry = entry.expect("dirent");
        let path = entry.path();
        if path.is_dir() {
            walk(&path, out);
        } else if path.is_file() {
            out.push(path);
        }
    }
}

#[test]
fn fixtures_are_free_of_secrets_and_pii() {
    let secret_re = regex::Regex::new(SECRET_PATTERN).expect("secret regex");
    let email_re = regex::Regex::new(EMAIL_PATTERN).expect("email regex");

    let mut files = Vec::new();
    walk(&fixtures_root(), &mut files);

    let mut violations: Vec<String> = Vec::new();
    for path in files {
        let Ok(body) = fs::read_to_string(&path) else {
            // Binary fixtures are fine — skip. None should exist today.
            continue;
        };
        if secret_re.is_match(&body) {
            violations.push(format!(
                "secret-like token in {} (pattern: {SECRET_PATTERN})",
                path.display()
            ));
        }
        if email_re.is_match(&body) {
            violations.push(format!(
                "strongdm email in {} (pattern: {EMAIL_PATTERN})",
                path.display()
            ));
        }
    }

    assert!(
        violations.is_empty(),
        "fixture redaction lint failed:\n  {}",
        violations.join("\n  ")
    );
}

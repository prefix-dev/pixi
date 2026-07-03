//! Adversarial stress tests: a corpus of valid-but-nasty TOML formatting
//! variants crossed with every editing operation. Three invariants must hold
//! for every combination:
//!
//! 1. the rendered document must re-parse as valid TOML
//! 2. the semantic content (keys/values and their order) must match a model
//! 3. comments must be preserved according to the crate's contract: a line
//!    comment dies exactly when the element it trails is removed, every other
//!    comment survives every operation, and no comment is ever duplicated
//!
//! Comment attribution is encoded in the comment text itself: a comment of
//! the form `# after <elem> ...` is a line comment trailing `<elem>` and dies
//! with it; every other comment text must survive.
//!
//! Failures are collected and reported all at once so a single run shows the
//! full damage.

use std::fmt::Write as _;

use toml_edit::{DocumentMut, Value};

/// Array corpus: every entry is a complete document with an array under the
/// key `x` holding a subset of the string elements "a", "b", "c", "d".
const ARRAY_CORPUS: &[&str] = &[
    // single line variations
    "x = [\"a\", \"b\", \"c\"]\n",
    "x = [ \"a\" , \"b\" ,\"c\" ]\n",
    "x = [\"a\",\"b\",\"c\",]\n",
    "x = [\"a\"]\n",
    "x = []\n",
    "x = [ ]\n",
    "x = [\"a\", \"b\", \"c\"] # doc comment\n",
    // multiline variations
    "x = [\n    \"a\",\n    \"b\",\n    \"c\",\n]\n",
    "x = [\n    \"a\",\n    \"b\",\n    \"c\"\n]\n",
    "x = [\n]\n",
    "x = [\n    \"a\",\n]\n",
    "x = [\"a\",\n    \"b\",\n    \"c\"]\n",
    "x = [\n  \"a\",\n  \"b\",\n  \"c\",\n  ]\n",
    "x = [\n\t\"a\",\n\t\"b\",\n\t\"c\",\n]\n",
    "x = [\n    \"a\", \"b\",\n    \"c\", \"d\",\n]\n",
    "x = [\n    \"a\",\n    \"b\",\n    \"c\",\n] # doc comment\n",
    // comments in every position
    "x = [\n    \"a\", # after a\n    \"b\",\n    \"c\",\n]\n",
    "x = [\n    \"a\",\n    \"b\", # after b\n    \"c\",\n]\n",
    "x = [\n    \"a\",\n    \"b\",\n    \"c\", # after c\n]\n",
    "x = [\n    \"a\",\n    \"b\",\n    \"c\" # after c no comma\n]\n",
    "x = [\n    # standalone before a\n    \"a\",\n    \"b\",\n    \"c\",\n]\n",
    "x = [\n    \"a\",\n    # standalone before b\n    \"b\",\n    \"c\",\n]\n",
    "x = [\n    \"a\",\n    \"b\",\n    # standalone before c\n    \"c\",\n]\n",
    "x = [\n    \"a\",\n    \"b\",\n    \"c\",\n    # standalone before bracket\n]\n",
    "x = [ # header\n    \"a\",\n    \"b\",\n    \"c\",\n]\n",
    "x = [\n    # only a comment\n]\n",
    "x = [\n    \"a\", # after a\n    # standalone before b\n    \"b\",\n    \"c\",\n]\n",
    "x = [\n    \"a\" # after a no comma\n    # standalone before bracket\n]\n",
    "x = [\"a\", \"b\", # after b same line\n    \"c\",\n]\n",
    "x = [\n    \"a\", # after a\n    \"b\", # after b\n    \"c\", # after c\n]\n",
    // newline before comma / comment between value and comma
    "x = [\"a\"\n    , \"b\", \"c\"]\n",
    "x = [\"a\" # after a before comma\n    , \"b\", \"c\"]\n",
    // CRLF line endings
    "x = [\r\n    \"a\",\r\n    \"b\",\r\n    \"c\",\r\n]\r\n",
    "x = [\r\n    \"a\",\r\n    # standalone before b\r\n    \"b\",\r\n    \"c\", # after c\r\n]\r\n",
    // non-string elements mixed in stay untouched by the model (we only
    // add/remove strings)
    "x = [\n    \"a\",\n    { channel = \"b\", priority = 1 },\n    \"c\",\n]\n",
    "x = [\n    \"a\",\n    123,\n    \"c\",\n]\n",
];

/// Inline-table corpus: every entry is a complete document with a container
/// under the key `x` holding a subset of the keys "a", "b", "c" (and for some
/// entries a dotted key "d.e").
const TABLE_CORPUS: &[&str] = &[
    // single line variations
    "x = { a = 1, b = 2, c = 3 }\n",
    "x = {a = 1,b = 2,c = 3}\n",
    "x = { a = 1 }\n",
    "x = {}\n",
    "x = { }\n",
    "x = { a = 1, b = 2 } # doc comment\n",
    // multiline variations
    "x = {\n    a = 1,\n    b = 2,\n    c = 3,\n}\n",
    "x = {\n    a = 1,\n    b = 2,\n    c = 3\n}\n",
    "x = {\n}\n",
    "x = {\n    a = 1,\n}\n",
    "x = { a = 1,\n    b = 2,\n    c = 3 }\n",
    "x = {\n  a = 1,\n  b = 2,\n  c = 3,\n  }\n",
    "x = {\n\ta = 1,\n\tb = 2,\n\tc = 3,\n}\n",
    "x = {\n    a = 1, b = 2,\n    c = 3,\n}\n",
    "x = {\n    a = 1,\n    b = 2,\n    c = 3,\n} # doc comment\n",
    // comments in every position
    "x = {\n    a = 1, # after a\n    b = 2,\n    c = 3,\n}\n",
    "x = {\n    a = 1,\n    b = 2, # after b\n    c = 3,\n}\n",
    "x = {\n    a = 1,\n    b = 2,\n    c = 3, # after c\n}\n",
    "x = {\n    a = 1,\n    b = 2,\n    c = 3 # after c no comma\n}\n",
    "x = {\n    # standalone before a\n    a = 1,\n    b = 2,\n    c = 3,\n}\n",
    "x = {\n    a = 1,\n    # standalone before b\n    b = 2,\n    c = 3,\n}\n",
    "x = {\n    a = 1,\n    b = 2,\n    # standalone before c\n    c = 3,\n}\n",
    "x = {\n    a = 1,\n    b = 2,\n    c = 3,\n    # standalone before brace\n}\n",
    "x = { # header\n    a = 1,\n    b = 2,\n    c = 3,\n}\n",
    "x = {\n    # only a comment\n}\n",
    "x = {\n    a = 1, # after a\n    # standalone before b\n    b = 2,\n    c = 3,\n}\n",
    "x = {\n    a = 1, # after a\n    b = 2, # after b\n    c = 3, # after c\n}\n",
    // dotted keys
    "x = { a = 1, d.e = 2, c = 3 }\n",
    "x = {\n    a = 1,\n    d.e = 2,\n    c = 3,\n}\n",
    "x = {\n    a = 1, # after a\n    d.e = 2,\n    c = 3,\n}\n",
    "x = {\n    # standalone before a\n    a = 1,\n    d.e = 2,\n}\n",
    "x = {\n    a = 1,\n    d.e = 2, # after d\n    c = 3,\n}\n",
    "x = {\n    a = 1,\n    d.e = 2, # after d\n}\n",
    "x = {\n    a = 1,\n    d.e = 2 # after d no comma\n}\n",
    "x = {\n    d.e = 2,\n}\n",
    // newline before comma
    "x = { a = 1\n    , b = 2, c = 3 }\n",
    // CRLF
    "x = {\r\n    a = 1,\r\n    b = 2,\r\n    c = 3,\r\n}\r\n",
    "x = {\r\n    a = 1,\r\n    # standalone before b\r\n    b = 2,\r\n    c = 3, # after c\r\n}\r\n",
    // regular table
    "[x]\na = 1\nb = 2\nc = 3\n",
    "[x]\na = 1 # after a\n# standalone before b\nb = 2\nc = 3\n",
    "[x]\n# standalone before a\na = 1\nb = 2\n",
    "[x]\na = 1\nd.e = 2\nc = 3\n",
    "[x]\na = 1\n\n# standalone before b\nb = 2\n",
];

struct Failure {
    corpus: String,
    operation: String,
    problem: String,
    output: String,
}

fn report(failures: &[Failure]) {
    if failures.is_empty() {
        return;
    }
    let mut message = format!("{} failing combination(s):\n", failures.len());
    for failure in failures {
        let _ = write!(
            message,
            "\n=== operation: {}\n--- input:\n{}\n--- problem: {}\n--- output:\n{}\n",
            failure.operation,
            failure.corpus.replace('\r', "<CR>"),
            failure.problem,
            failure.output.replace('\r', "<CR>"),
        );
    }
    panic!("{message}");
}

/// Extracts the comment texts of a document. None of the corpus values
/// contain a `#`, so scanning each line for the first `#` is exact.
fn comment_texts(text: &str) -> Vec<String> {
    let mut comments: Vec<String> = text
        .lines()
        .filter_map(|line| {
            line.find('#')
                .map(|index| line[index..].trim_end().to_string())
        })
        .collect();
    comments.sort();
    comments
}

/// The comments that must be present after removing the elements in
/// `removed`: a comment `# after <elem> ...` dies with `<elem>`, everything
/// else survives.
fn expected_comments(corpus: &str, removed: &[&String]) -> Vec<String> {
    comment_texts(corpus)
        .into_iter()
        .filter(|comment| {
            let Some(rest) = comment.strip_prefix("# after ") else {
                return true;
            };
            let element = rest.split_whitespace().next().unwrap_or("");
            !removed.iter().any(|target| target.as_str() == element)
        })
        .collect()
}

/// Checks the three invariants and records a failure if any is broken.
fn check(
    failures: &mut Vec<Failure>,
    corpus: &str,
    operation: &str,
    doc: &DocumentMut,
    expected_strings: &[String],
    removed: &[&String],
    actual_strings: impl Fn(&DocumentMut) -> Vec<String>,
) {
    let output = doc.to_string();
    let reparsed = match output.parse::<DocumentMut>() {
        Err(error) => {
            failures.push(Failure {
                corpus: corpus.to_string(),
                operation: operation.to_string(),
                problem: format!("output is not valid TOML: {error}"),
                output,
            });
            return;
        }
        Ok(reparsed) => reparsed,
    };

    let actual = actual_strings(&reparsed);
    if actual != expected_strings {
        failures.push(Failure {
            corpus: corpus.to_string(),
            operation: operation.to_string(),
            problem: format!("expected {expected_strings:?}, got {actual:?}"),
            output,
        });
        return;
    }

    let expected = expected_comments(corpus, removed);
    let actual = comment_texts(&output);
    if actual != expected {
        failures.push(Failure {
            corpus: corpus.to_string(),
            operation: operation.to_string(),
            problem: format!("expected comments {expected:?}, got {actual:?}"),
            output,
        });
    }
}

fn array_strings(doc: &DocumentMut) -> Vec<String> {
    doc["x"]
        .as_array()
        .expect("x must still be an array")
        .iter()
        .map(|value| match value.as_str() {
            Some(s) => s.to_string(),
            None => "<non-string>".to_string(),
        })
        .collect()
}

fn table_keys(doc: &DocumentMut) -> Vec<String> {
    doc["x"]
        .as_table_like()
        .expect("x must still be table-like")
        .iter()
        .map(|(key, _)| key.to_string())
        .collect()
}

/// The values pushed or inserted into arrays: a plain string and an inline
/// table as written for a prioritized channel.
fn new_array_values() -> Vec<(&'static str, Value, String)> {
    let table: Value = "{ channel = \"new\", priority = 1 }".parse().unwrap();
    vec![
        ("string", Value::from("new"), "new".to_string()),
        ("inline table", table, "<non-string>".to_string()),
    ]
}

#[test]
fn array_operations_on_corpus() {
    let mut failures = Vec::new();

    for corpus in ARRAY_CORPUS {
        let parse = || corpus.parse::<DocumentMut>().expect("corpus must be valid");
        let initial = array_strings(&parse());
        let non_string = "<non-string>".to_string();

        for (kind, value, model_value) in new_array_values() {
            // push
            let mut doc = parse();
            pixi_toml_edit::push_array_element(doc["x"].as_array_mut().unwrap(), value.clone());
            let mut expected = initial.clone();
            expected.push(model_value.clone());
            check(
                &mut failures,
                corpus,
                &format!("push {kind}"),
                &doc,
                &expected,
                &[],
                array_strings,
            );

            // insert at every position including one past the end
            for index in 0..=initial.len() {
                let mut doc = parse();
                pixi_toml_edit::insert_array_element(
                    doc["x"].as_array_mut().unwrap(),
                    index,
                    value.clone(),
                );
                let mut expected = initial.clone();
                expected.insert(index, model_value.clone());
                check(
                    &mut failures,
                    corpus,
                    &format!("insert {kind} at {index}"),
                    &doc,
                    &expected,
                    &[],
                    array_strings,
                );
            }
        }

        // remove each single element
        for target in &initial {
            if target == &non_string {
                continue;
            }
            let mut doc = parse();
            pixi_toml_edit::retain_array_elements(doc["x"].as_array_mut().unwrap(), |value| {
                value.as_str() != Some(target)
            });
            let expected: Vec<String> = initial.iter().filter(|s| *s != target).cloned().collect();
            check(
                &mut failures,
                corpus,
                &format!("remove {target}"),
                &doc,
                &expected,
                &[target],
                array_strings,
            );
        }

        // remove every pair of elements
        for first in &initial {
            for second in &initial {
                if first >= second || first == &non_string || second == &non_string {
                    continue;
                }
                let mut doc = parse();
                pixi_toml_edit::retain_array_elements(doc["x"].as_array_mut().unwrap(), |value| {
                    value.as_str() != Some(first) && value.as_str() != Some(second)
                });
                let expected: Vec<String> = initial
                    .iter()
                    .filter(|s| *s != first && *s != second)
                    .cloned()
                    .collect();
                check(
                    &mut failures,
                    corpus,
                    &format!("remove {first} and {second}"),
                    &doc,
                    &expected,
                    &[first, second],
                    array_strings,
                );
            }
        }

        // remove everything
        {
            let mut doc = parse();
            pixi_toml_edit::retain_array_elements(doc["x"].as_array_mut().unwrap(), |_| false);
            let removed: Vec<&String> = initial.iter().collect();
            check(
                &mut failures,
                corpus,
                "remove everything",
                &doc,
                &[],
                &removed,
                array_strings,
            );
        }

        // remove one element, then push a new one: compounding edits
        for target in &initial {
            if target == &non_string {
                continue;
            }
            let mut doc = parse();
            pixi_toml_edit::retain_array_elements(doc["x"].as_array_mut().unwrap(), |value| {
                value.as_str() != Some(target)
            });
            pixi_toml_edit::push_array_element(
                doc["x"].as_array_mut().unwrap(),
                Value::from("new"),
            );
            let mut expected: Vec<String> =
                initial.iter().filter(|s| *s != target).cloned().collect();
            expected.push("new".to_string());
            check(
                &mut failures,
                corpus,
                &format!("remove {target} then push"),
                &doc,
                &expected,
                &[target],
                array_strings,
            );
        }

        // push then remove the pushed element: must round-trip semantically
        // and keep every comment
        {
            let mut doc = parse();
            pixi_toml_edit::push_array_element(
                doc["x"].as_array_mut().unwrap(),
                Value::from("new"),
            );
            pixi_toml_edit::retain_array_elements(doc["x"].as_array_mut().unwrap(), |value| {
                value.as_str() != Some("new")
            });
            check(
                &mut failures,
                corpus,
                "push then remove pushed",
                &doc,
                &initial,
                &[],
                array_strings,
            );
        }
    }

    report(&failures);
}

#[test]
fn table_operations_on_corpus() {
    let mut failures = Vec::new();

    for corpus in TABLE_CORPUS {
        let parse = || corpus.parse::<DocumentMut>().expect("corpus must be valid");
        let initial = table_keys(&parse());

        // upsert a new key
        {
            let mut doc = parse();
            pixi_toml_edit::upsert_entry(&mut doc["x"], "new", Value::from(9)).unwrap();
            let mut expected = initial.clone();
            expected.push("new".to_string());
            check(
                &mut failures,
                corpus,
                "upsert new",
                &doc,
                &expected,
                &[],
                table_keys,
            );
        }

        // overwrite each existing key
        for target in &initial {
            let mut doc = parse();
            pixi_toml_edit::upsert_entry(&mut doc["x"], target, Value::from(9)).unwrap();
            check(
                &mut failures,
                corpus,
                &format!("overwrite {target}"),
                &doc,
                &initial,
                &[],
                table_keys,
            );
        }

        // remove each existing key
        for target in &initial {
            let mut doc = parse();
            let removed = pixi_toml_edit::remove_entry(&mut doc["x"], target).unwrap();
            let expected: Vec<String> = initial.iter().filter(|k| *k != target).cloned().collect();
            if removed.is_none() {
                failures.push(Failure {
                    corpus: corpus.to_string(),
                    operation: format!("remove {target}"),
                    problem: "remove_entry returned None for an existing key".to_string(),
                    output: doc.to_string(),
                });
                continue;
            }
            check(
                &mut failures,
                corpus,
                &format!("remove {target}"),
                &doc,
                &expected,
                &[target],
                table_keys,
            );
        }

        // remove every pair of keys
        for first in &initial {
            for second in &initial {
                if first >= second {
                    continue;
                }
                let mut doc = parse();
                pixi_toml_edit::remove_entry(&mut doc["x"], first).unwrap();
                pixi_toml_edit::remove_entry(&mut doc["x"], second).unwrap();
                let expected: Vec<String> = initial
                    .iter()
                    .filter(|k| *k != first && *k != second)
                    .cloned()
                    .collect();
                check(
                    &mut failures,
                    corpus,
                    &format!("remove {first} and {second}"),
                    &doc,
                    &expected,
                    &[first, second],
                    table_keys,
                );
            }
        }

        // remove everything
        {
            let mut doc = parse();
            for target in &initial {
                pixi_toml_edit::remove_entry(&mut doc["x"], target).unwrap();
            }
            let removed: Vec<&String> = initial.iter().collect();
            check(
                &mut failures,
                corpus,
                "remove everything",
                &doc,
                &[],
                &removed,
                table_keys,
            );
        }

        // remove one key, then upsert a new one: compounding edits
        for target in &initial {
            let mut doc = parse();
            pixi_toml_edit::remove_entry(&mut doc["x"], target).unwrap();
            pixi_toml_edit::upsert_entry(&mut doc["x"], "new", Value::from(9)).unwrap();
            let mut expected: Vec<String> =
                initial.iter().filter(|k| *k != target).cloned().collect();
            expected.push("new".to_string());
            check(
                &mut failures,
                corpus,
                &format!("remove {target} then upsert new"),
                &doc,
                &expected,
                &[target],
                table_keys,
            );
        }

        // upsert then remove the new key: must round-trip semantically and
        // keep every comment
        {
            let mut doc = parse();
            pixi_toml_edit::upsert_entry(&mut doc["x"], "new", Value::from(9)).unwrap();
            pixi_toml_edit::remove_entry(&mut doc["x"], "new").unwrap();
            check(
                &mut failures,
                corpus,
                "upsert then remove new",
                &doc,
                &initial,
                &[],
                table_keys,
            );
        }
    }

    report(&failures);
}

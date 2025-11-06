//! Shell script parsing utilities using ast-grep for syntax analysis.
//!
//! This crate provides functionality to parse and split bash scripts by top-level operators.

use ast_grep_core::{AstGrep, Doc, Language};
use thiserror::Error;

/// Errors that can occur during shell script parsing.
#[derive(Debug, Error)]
pub enum ShellParseError {
    /// The shell script has invalid syntax.
    #[error("Invalid shell syntax: {0}")]
    InvalidSyntax(String),

    /// An error occurred during parsing.
    #[error("Parse error: {0}")]
    ParseError(String),
}

/// Bash language implementation for ast-grep.
#[derive(Clone)]
struct BashLanguage;

impl Language for BashLanguage {
    fn get_ts_language(&self) -> ast_grep_core::language::TSLanguage {
        tree_sitter_bash::LANGUAGE.into()
    }
}

/// Splits a bash script string into multiple command strings by top-level `&&` operators.
///
/// This function parses the bash script and identifies command lists separated by `&&` at the
/// top level (not nested within subshells, functions, or other constructs).
///
/// # Arguments
///
/// * `script` - The bash script string to split
///
/// # Returns
///
/// A `Result` containing a vector of command strings, or a `ShellParseError` if parsing fails.
///
/// # Examples
///
/// ```
/// use vite_shell::split_by_and;
///
/// let script = "npm run build && npm test";
/// let commands = split_by_and(script).unwrap();
/// assert_eq!(commands, vec!["npm run build", "npm test"]);
/// ```
///
/// ```
/// use vite_shell::split_by_and;
///
/// let script = "echo 'hello' && echo 'world' && echo 'rust'";
/// let commands = split_by_and(script).unwrap();
/// assert_eq!(commands, vec!["echo 'hello'", "echo 'world'", "echo 'rust'"]);
/// ```
pub fn split_by_and(script: &str) -> Result<Vec<String>, ShellParseError> {
    let grep = AstGrep::new(script, BashLanguage);
    let root = grep.root();

    // Split by top-level && operators
    let commands = split_list_by_operator(&root, "&&", script);

    if commands.is_empty() {
        // If no && operators found, return the entire script as a single command
        Ok(vec![script.trim().to_string()])
    } else {
        Ok(commands)
    }
}

/// Splits a node by a specific operator at the top level only.
///
/// This function walks the AST and splits only at the specified operator,
/// but handles nested lists that ALSO have the same operator (continuing the chain).
fn split_list_by_operator<D: Doc>(
    node: &ast_grep_core::Node<D>,
    operator: &str,
    script: &str,
) -> Vec<String> {
    let kind = node.kind();

    // Only process "list" nodes which contain operator sequences
    if kind.as_ref() != "list" {
        // For program nodes, check children
        if kind.as_ref() == "program" {
            for child in node.children() {
                let results = split_list_by_operator(&child, operator, script);
                if !results.is_empty() {
                    return results;
                }
            }
        }
        return Vec::new();
    }

    // We have a list node - check if it contains our target operator AT THIS LEVEL
    let children: Vec<_> = node.children().collect();
    let has_target_operator = children.iter().any(|c| c.kind().as_ref() == operator);

    if !has_target_operator {
        // No target operator at this level
        return Vec::new();
    }

    // Found target operator at this level - split by it
    // If we encounter a nested list, check if it's ONLY our operator (continue chain)
    // or if it has OTHER operators (treat as atomic)
    let mut commands = Vec::new();
    let mut current_start: Option<usize> = None;
    let mut current_end: Option<usize> = None;

    for child in &children {
        let child_kind = child.kind();

        if child_kind.as_ref() == operator {
            // Hit the operator - save current command if we have one
            if let (Some(start), Some(end)) = (current_start, current_end) {
                commands.push(script[start..end].trim().to_string());
            }
            // Reset for next command
            current_start = None;
            current_end = None;
        } else if child_kind.as_ref() == "list" {
            // Nested list - check what operators it contains
            let nested_children: Vec<_> = child.children().collect();
            let has_our_operator = nested_children.iter().any(|c| c.kind().as_ref() == operator);
            let has_other_operator = nested_children.iter().any(|c| {
                let k = c.kind();
                k.as_ref() == "||" || k.as_ref() == ";" || k.as_ref() == "|" || k.as_ref() == "&"
            });

            if has_our_operator && !has_other_operator {
                // This nested list ONLY has our operator - it's a continuation of the chain
                // Recursively process it and merge
                let nested_results = split_list_by_operator(child, operator, script);
                if !nested_results.is_empty() {
                    if let (Some(start), Some(_)) = (current_start, current_end) {
                        // Merge first result with accumulated parts
                        let prefix = script[start..child.range().start].trim();
                        if !prefix.is_empty() {
                            commands.push(format!("{} && {}", prefix, nested_results[0]));
                            commands.extend(nested_results.into_iter().skip(1));
                        } else {
                            commands.extend(nested_results);
                        }
                        current_start = None;
                        current_end = None;
                    } else {
                        commands.extend(nested_results);
                    }
                } else {
                    // Shouldn't happen, but treat as atomic
                    let range = child.range();
                    if current_start.is_none() {
                        current_start = Some(range.start);
                    }
                    current_end = Some(range.end);
                }
            } else {
                // Nested list has other operators or no our operator - treat as atomic
                let range = child.range();
                if current_start.is_none() {
                    current_start = Some(range.start);
                }
                current_end = Some(range.end);
            }
        } else {
            // Part of a command
            let range = child.range();
            if current_start.is_none() {
                current_start = Some(range.start);
            }
            current_end = Some(range.end);
        }
    }

    // Don't forget the last command
    if let (Some(start), Some(end)) = (current_start, current_end) {
        commands.push(script[start..end].trim().to_string());
    }

    commands
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_simple_split() {
        let script = "cmd1 && cmd2";
        let commands = split_by_and(script).unwrap();
        assert_eq!(commands, vec!["cmd1", "cmd2"]);
    }

    #[test]
    fn test_or_then_and() {
        // || and && have same precedence, left-associative
        // So this parses as: (cmd0 || cmd1) && cmd2
        let script = "cmd0 || cmd1 && cmd2";
        let commands = split_by_and(script).unwrap();
        assert_eq!(commands, vec!["cmd0 || cmd1", "cmd2"]);
    }

    #[test]
    fn test_and_then_or() {
        // This parses as: (a && b) || c
        // The && is nested in a list inside an || context
        // Since there's no && at the top level (only ||), we don't split
        let script = "a && b || c";
        let commands = split_by_and(script).unwrap();
        // No top-level &&, so return the whole thing
        assert_eq!(commands, vec!["a && b || c"]);
    }

    #[test]
    fn test_mixed_operators() {
        // Parses as: ((a && b) || c) && d
        let script = "a && b || c && d";
        let commands = split_by_and(script).unwrap();
        // There IS a top-level && (between "((a && b) || c)" and "d")
        // So we split there, treating the left side as atomic
        assert_eq!(commands, vec!["a && b || c", "d"]);
    }

    #[test]
    fn test_only_or() {
        // Only || operators, no splitting
        let script = "cmd1 || cmd2 || cmd3";
        let commands = split_by_and(script).unwrap();
        assert_eq!(commands, vec!["cmd1 || cmd2 || cmd3"]);
    }

    #[test]
    fn test_multiple_and() {
        let script = "a && b && c";
        let commands = split_by_and(script).unwrap();
        assert_eq!(commands, vec!["a", "b", "c"]);
    }

    #[test]
    fn test_no_and() {
        let script = "single command";
        let commands = split_by_and(script).unwrap();
        assert_eq!(commands, vec!["single command"]);
    }

    #[test]
    fn test_with_whitespace() {
        let script = "  cmd1  &&  cmd2  ";
        let commands = split_by_and(script).unwrap();
        assert_eq!(commands, vec!["cmd1", "cmd2"]);
    }

    #[test]
    fn test_complex_commands() {
        let script = "npm run build && npm test --coverage && echo 'done'";
        let commands = split_by_and(script).unwrap();
        assert_eq!(commands, vec!["npm run build", "npm test --coverage", "echo 'done'"]);
    }

    #[test]
    fn test_subshell_with_and() {
        let script = "(cmd1 && cmd2) && cmd3";
        let commands = split_by_and(script).unwrap();
        // Should split at the top-level &&, keeping the subshell intact
        assert_eq!(commands, vec!["(cmd1 && cmd2)", "cmd3"]);
    }

    #[test]
    fn test_with_pipes() {
        let script = "cat file.txt | grep pattern && echo 'found'";
        let commands = split_by_and(script).unwrap();
        assert_eq!(commands, vec!["cat file.txt | grep pattern", "echo 'found'"]);
    }

    #[test]
    fn test_with_newlines() {
        let script = "cmd1 &&\n  cmd2 &&\n  cmd3";
        let commands = split_by_and(script).unwrap();
        assert_eq!(commands, vec!["cmd1", "cmd2", "cmd3"]);
    }
}

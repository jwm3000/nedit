//! "Smart typing" for LaTeX (like Overleaf): bracket pairing, automatic `\end{…}`,
//! list continuation and indentation on Enter. Pure string functions, unit-tested.

/// Result of a smart edit: new text and cursor (char index).
#[derive(Debug, PartialEq)]
pub struct Edit {
    pub text: String,
    pub cursor: usize,
}

fn chars(s: &str) -> Vec<char> {
    s.chars().collect()
}

fn line_start(c: &[char], p: usize) -> usize {
    let mut i = p.min(c.len());
    while i > 0 && c[i - 1] != '\n' {
        i -= 1;
    }
    i
}

fn line_end(c: &[char], p: usize) -> usize {
    let mut i = p.min(c.len());
    while i < c.len() && c[i] != '\n' {
        i += 1;
    }
    i
}

fn indent_of(c: &[char], p: usize) -> String {
    let s = line_start(c, p);
    c[s..].iter().take_while(|ch| **ch == ' ' || **ch == '\t').collect()
}

fn build(c: &[char], a: usize, b: usize, ins: &str, caret: usize) -> Edit {
    let mut t: String = c[..a].iter().collect();
    t.push_str(ins);
    t.extend(c[b..].iter());
    Edit { text: t, cursor: a + caret }
}

/// Body that is placed between `\begin{env}` and `\end{env}`; `$0` marks the cursor.
pub fn env_body(env: &str) -> &'static str {
    match env {
        "itemize" | "enumerate" | "description" | "tugitemize" | "tugenumerate" | "compactitem" | "compactenum" => "\\item $0",
        "figure" | "figure*" => "\\centering\n\\includegraphics[width=0.8\\linewidth]{$0}\n\\caption{}\n\\label{fig:}",
        "table" | "table*" => "\\centering\n\\caption{$0}\n\\label{tab:}\n\\begin{tabular}{lll}\n  \\toprule\n  A & B & C \\\\\n  \\midrule\n  1 & 2 & 3 \\\\\n  \\bottomrule\n\\end{tabular}",
        "equation" | "align" | "gather" | "multline" => "$0\n\\label{eq:}",
        "columns" | "tugcolumns" => "\\begin{column}{0.48\\textwidth}\n  $0\n\\end{column}\n\\begin{column}{0.48\\textwidth}\n  \n\\end{column}",
        _ => "$0",
    }
}

/// Text inserted after `\begin{env}`: an indented body and the matching `\end{env}`.
/// Returns (text, caret offset in chars).
pub fn env_block(env: &str, indent: &str) -> (String, usize) {
    let inner = format!("{indent}  ");
    let body: Vec<String> = env_body(env).split('\n').map(|l| format!("{inner}{l}")).collect();
    let s = format!("\n{}\n{indent}\\end{{{env}}}", body.join("\n"));
    let caret_b = s.find("$0").unwrap_or(0);
    let caret = s[..caret_b].chars().count();
    (s.replacen("$0", "", 1), caret)
}

/// Is there already an unmatched `\end{env}` after position `p`?
fn has_open_end(c: &[char], p: usize, env: &str) -> bool {
    let rest: String = c[p.min(c.len())..].iter().collect();
    let begin = format!("\\begin{{{env}}}");
    let end = format!("\\end{{{env}}}");
    let mut depth = 0i32;
    let mut i = 0;
    while i < rest.len() {
        let r = &rest[i..];
        if r.starts_with(&begin) {
            depth += 1;
            i += begin.len();
        } else if r.starts_with(&end) {
            if depth == 0 {
                return true;
            }
            depth -= 1;
            i += end.len();
        } else {
            i += r.chars().next().map(|ch| ch.len_utf8()).unwrap_or(1);
        }
    }
    false
}

/// If the text before `p` ends with `\begin{env}` (optionally with `[opt]`/`{arg}` after),
/// return the environment name.
fn begin_before(c: &[char], p: usize) -> Option<String> {
    let s = line_start(c, p);
    let before: String = c[s..p].iter().collect();
    let re = regex::Regex::new(r"\\begin\{([A-Za-z*]+)\}(?:\[[^\]]*\]|\{[^{}]*\})*\s*$").unwrap();
    re.captures(&before).map(|m| m[1].to_string())
}

fn is_escaped(c: &[char], p: usize) -> bool {
    // odd number of backslashes before p
    let mut n = 0;
    let mut i = p;
    while i > 0 && c[i - 1] == '\\' {
        n += 1;
        i -= 1;
    }
    n % 2 == 1
}

/// Typed a single character. `sel` = selected range (a, b) or (p, p).
pub fn on_char(text: &str, sel: (usize, usize), ch: char) -> Option<Edit> {
    let c = chars(text);
    let (a, b) = (sel.0.min(sel.1), sel.0.max(sel.1));
    let next = c.get(b).copied();
    match ch {
        '{' | '[' if !is_escaped(&c, a) => {
            let close = if ch == '{' { '}' } else { ']' };
            if b > a {
                // wrap the selection
                let inner: String = c[a..b].iter().collect();
                return Some(build(&c, a, b, &format!("{ch}{inner}{close}"), inner.chars().count() + 2));
            }
            // only pair when the next char doesn't continue a word
            if next.is_some_and(|n| n.is_alphanumeric()) {
                return None;
            }
            if ch == '[' {
                // brackets only after a command name (\section[, \begin{x}[ …), not in prose/math
                let s = line_start(&c, a);
                let before: String = c[s..a].iter().collect();
                if !regex::Regex::new(r"(\\[A-Za-z]+\*?|\})$").unwrap().is_match(&before) {
                    return None;
                }
            }
            Some(build(&c, a, b, &format!("{ch}{close}"), 1))
        }
        '}' | ']' if a == b => {
            let skip = next == Some(ch);
            // closing `\begin{env` → add the matching \end automatically
            if ch == '}' {
                let s = line_start(&c, a);
                let before: String = c[s..a].iter().collect();
                if let Some(m) = regex::Regex::new(r"\\begin\{([A-Za-z*]+)$").unwrap().captures(&before) {
                    let env = m[1].to_string();
                    let after = if skip { a + 1 } else { a };
                    let mut base = c.clone();
                    if !skip {
                        base.insert(a, '}');
                    }
                    // rest of the line after the brace must be empty (else just close)
                    let le = line_end(&base, after + if skip { 0 } else { 1 });
                    let rest_line: String = base[a + 1..le].iter().collect();
                    if rest_line.trim().is_empty() && !has_open_end(&base, a + 1, &env) {
                        let ind = indent_of(&base, a);
                        let (block, caret) = env_block(&env, &ind);
                        let mut e = build(&base, a + 1, le, &block, caret);
                        // keep anything that followed on the line (whitespace only) out
                        e.cursor = a + 1 + caret;
                        return Some(e);
                    }
                    if skip {
                        return Some(Edit { text: text.to_string(), cursor: a + 1 });
                    }
                    return None;
                }
            }
            if skip {
                return Some(Edit { text: text.to_string(), cursor: a + 1 });
            }
            None
        }
        _ => None,
    }
}

/// Backspace between an empty pair `{}` / `[]` deletes both.
pub fn on_backspace(text: &str, p: usize) -> Option<Edit> {
    let c = chars(text);
    if p == 0 || p >= c.len() {
        return None;
    }
    let pair = matches!((c[p - 1], c[p]), ('{', '}') | ('[', ']'));
    if pair && !is_escaped(&c, p - 1) {
        return Some(build(&c, p - 1, p + 1, "", 0));
    }
    None
}

/// Enter: keep indentation, continue `\item` lists, open environments.
pub fn on_enter(text: &str, p: usize) -> Edit {
    let c = chars(text);
    let ind = indent_of(&c, p);
    let s = line_start(&c, p);
    let before: String = c[s..p].iter().collect();
    let after_line: String = c[p..line_end(&c, p)].iter().collect();

    // \begin{env} on this line and no \end yet → create it
    if after_line.trim().is_empty() {
        if let Some(env) = begin_before(&c, p) {
            if !has_open_end(&c, p, &env) {
                let le = line_end(&c, p);
                let (block, caret) = env_block(&env, &ind);
                return build(&c, p, le, &block, caret);
            }
            // \end exists (e.g. on the next line): open an indented line in between
            let ins = format!("\n{ind}  ");
            let n = ins.chars().count();
            return build(&c, p, p, &ins, n);
        }
    }
    // list item continuation
    let t = before.trim_start();
    if t.starts_with("\\item") && after_line.trim().is_empty() {
        let content = t.trim_start_matches("\\item").trim();
        if content.is_empty() || content.starts_with('<') && content.ends_with('>') && !content.contains(' ') {
            // empty item: end the list item (remove it, leave an empty line)
            let le = line_end(&c, p);
            return build(&c, s, le, &ind, ind.chars().count());
        }
        let ins = format!("\n{ind}\\item ");
        let n = ins.chars().count();
        return build(&c, p, p, &ins, n);
    }
    let ins = format!("\n{ind}");
    let n = ins.chars().count();
    build(&c, p, p, &ins, n)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(t: &str) -> (String, usize) {
        let p = t.find('|').unwrap();
        (t.replacen('|', "", 1), t[..p].chars().count())
    }

    fn show(e: &Edit) -> String {
        let mut s: Vec<char> = e.text.chars().collect();
        s.insert(e.cursor, '|');
        s.into_iter().collect()
    }

    #[test]
    fn pairs_and_skips() {
        let (t, p) = at("\\textbf|");
        assert_eq!(show(&on_char(&t, (p, p), '{').unwrap()), "\\textbf{|}");
        let (t, p) = at("\\textbf{x|}");
        assert_eq!(show(&on_char(&t, (p, p), '}').unwrap()), "\\textbf{x}|");
        let (t, p) = at("a \\|");
        assert!(on_char(&t, (p, p), '{').is_none(), "escaped brace");
        let (t, p) = at("{|}");
        assert_eq!(show(&on_backspace(&t, p).unwrap()), "|");
        // [ only after commands
        let (t, p) = at("x = |");
        assert!(on_char(&t, (p, p), '[').is_none());
        let (t, p) = at("\\section|");
        assert_eq!(show(&on_char(&t, (p, p), '[').unwrap()), "\\section[|]");
        // wrap selection
        assert_eq!(show(&on_char("abc", (0, 3), '{').unwrap()), "{abc}|");
    }

    #[test]
    fn auto_end() {
        // typed `}` (skipping the auto-paired one) completes the environment
        let (t, p) = at("  \\begin{itemize|}");
        assert_eq!(show(&on_char(&t, (p, p), '}').unwrap()), "  \\begin{itemize}\n    \\item |\n  \\end{itemize}");
        // without auto pair
        let (t, p) = at("\\begin{center|");
        assert_eq!(show(&on_char(&t, (p, p), '}').unwrap()), "\\begin{center}\n  |\n\\end{center}");
        // existing \end → just close
        let (t, p) = at("\\begin{center|}\nx\n\\end{center}");
        assert_eq!(show(&on_char(&t, (p, p), '}').unwrap()), "\\begin{center}|\nx\n\\end{center}");
        // Enter after \begin{env}
        let (t, p) = at("\\begin{equation}|");
        assert_eq!(show(&on_enter(&t, p)), "\\begin{equation}\n  |\n  \\label{eq:}\n\\end{equation}");
        // Enter with existing end: indented line in between
        let (t, p) = at("\\begin{center}|\n\\end{center}");
        assert_eq!(show(&on_enter(&t, p)), "\\begin{center}\n  |\n\\end{center}");
        // frame with title argument
        let (t, p) = at("\\begin{frame}{Titel}|");
        assert_eq!(show(&on_enter(&t, p)), "\\begin{frame}{Titel}\n  |\n\\end{frame}");
    }

    #[test]
    fn lists_and_indent() {
        let (t, p) = at("  \\item eins|");
        assert_eq!(show(&on_enter(&t, p)), "  \\item eins\n  \\item |");
        let (t, p) = at("  \\item eins\n  \\item |");
        assert_eq!(show(&on_enter(&t, p)), "  \\item eins\n  |");
        let (t, p) = at("    text|");
        assert_eq!(show(&on_enter(&t, p)), "    text\n    |");
    }
}

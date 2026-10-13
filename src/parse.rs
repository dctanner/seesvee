//! A fast, forgiving RFC 4180 parser that reports exactly where a file is
//! malformed instead of giving up on it.

use std::borrow::Cow;
use std::fmt;

use memchr::{memchr, memchr3};

use crate::store::{RowId, Store, StoreBuilder};

/// Only the first issues are kept; the total is still counted.
const MAX_ISSUES: usize = 10_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineEnding {
    Lf,
    CrLf,
    Cr,
}

impl LineEnding {
    pub fn as_str(self) -> &'static str {
        match self {
            LineEnding::Lf => "\n",
            LineEnding::CrLf => "\r\n",
            LineEnding::Cr => "\r",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            LineEnding::Lf => "LF",
            LineEnding::CrLf => "CRLF",
            LineEnding::Cr => "CR",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Encoding {
    Utf8,
    Utf16Le,
    Utf16Be,
    Windows1252,
}

impl Encoding {
    pub fn label(self) -> &'static str {
        match self {
            Encoding::Utf8 => "UTF-8",
            Encoding::Utf16Le => "UTF-16 LE",
            Encoding::Utf16Be => "UTF-16 BE",
            Encoding::Windows1252 => "Windows-1252",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum IssueKind {
    /// The file is not valid UTF-8 and was decoded with a fallback encoding.
    InvalidUtf8 { fallback: Encoding },
    /// A quoted field never closes. It was read as plain text up to the end of its line.
    UnterminatedQuote,
    /// A `"` appears inside a field that is not quoted.
    StrayQuote,
    /// Characters follow a closing quote before the next delimiter.
    TextAfterClosingQuote,
    /// A row has a different number of fields than the header.
    FieldCount {
        expected: usize,
        found: usize,
        /// First line of a quoted field in this row that spans several lines,
        /// which usually means a quote is unbalanced.
        multiline_from: Option<u32>,
    },
    /// Two columns share a name.
    DuplicateHeader { name: String },
}

#[derive(Clone, Debug, PartialEq)]
pub struct Issue {
    /// 1-based physical line in the file.
    pub line: u32,
    /// Data row the issue belongs to, or `None` for the header.
    pub row: Option<RowId>,
    /// 0-based field within the record.
    pub field: Option<u32>,
    pub kind: IssueKind,
}

impl Issue {
    pub fn is_error(&self) -> bool {
        !matches!(self.kind, IssueKind::DuplicateHeader { .. })
    }
}

impl fmt::Display for Issue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.kind {
            IssueKind::InvalidUtf8 { fallback } => write!(
                f,
                "Invalid UTF-8 byte sequence; the file was read as {}",
                fallback.label()
            ),
            IssueKind::UnterminatedQuote => write!(
                f,
                "Quoted field is never closed; read as plain text to the end of the line"
            ),
            IssueKind::StrayQuote => write!(f, "Unexpected quote inside an unquoted field"),
            IssueKind::TextAfterClosingQuote => {
                write!(f, "Unexpected text after a closing quote")
            }
            IssueKind::FieldCount {
                expected,
                found,
                multiline_from,
            } => {
                let noun = |n: usize| if n == 1 { "field" } else { "fields" };
                if found > expected {
                    write!(
                        f,
                        "Expected {expected} {}, found {found}; a value may contain an unquoted delimiter",
                        noun(*expected)
                    )?;
                } else {
                    write!(
                        f,
                        "Expected {expected} {}, found {found}; the row may be missing delimiters",
                        noun(*expected)
                    )?;
                }
                if let Some(from) = multiline_from {
                    write!(
                        f,
                        " (a quoted value starting on line {from} spans several lines, check for an unbalanced quote)"
                    )?;
                }
                Ok(())
            }
            IssueKind::DuplicateHeader { name } => {
                write!(f, "Duplicate column name \"{name}\"")
            }
        }
    }
}

pub struct Parsed {
    pub headers: Vec<String>,
    pub store: Store,
    pub delimiter: u8,
    pub line_ending: LineEnding,
    pub encoding: Encoding,
    pub has_bom: bool,
    pub issues: Vec<Issue>,
    pub issue_count: usize,
}

pub fn parse_bytes(bytes: &[u8], delimiter: Option<u8>) -> Parsed {
    let (text, encoding, has_bom, utf8_error_at) = decode(bytes);
    let delimiter = delimiter.unwrap_or_else(|| sniff_delimiter(&text));
    let mut parsed = parse_str(&text, delimiter);
    parsed.encoding = encoding;
    parsed.has_bom = has_bom;
    if let Some(offset) = utf8_error_at {
        let line = bytes[..offset].iter().filter(|&&b| b == b'\n').count() as u32 + 1;
        parsed.issue_count += 1;
        parsed.issues.insert(
            0,
            Issue {
                line,
                row: None,
                field: None,
                kind: IssueKind::InvalidUtf8 { fallback: encoding },
            },
        );
    }
    parsed
}

/// Decodes raw file bytes. Returns the text, the encoding used, whether a BOM
/// was present and, for invalid UTF-8, the offset of the first bad byte.
fn decode(bytes: &[u8]) -> (Cow<'_, str>, Encoding, bool, Option<usize>) {
    if let Some(rest) = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
        return match std::str::from_utf8(rest) {
            Ok(s) => (Cow::Borrowed(s), Encoding::Utf8, true, None),
            Err(e) => (
                Cow::Owned(decode_windows_1252(rest)),
                Encoding::Windows1252,
                true,
                Some(e.valid_up_to() + 3),
            ),
        };
    }
    if let Some(rest) = bytes.strip_prefix(&[0xFF, 0xFE]) {
        return (
            Cow::Owned(decode_utf16(rest, false)),
            Encoding::Utf16Le,
            true,
            None,
        );
    }
    if let Some(rest) = bytes.strip_prefix(&[0xFE, 0xFF]) {
        return (
            Cow::Owned(decode_utf16(rest, true)),
            Encoding::Utf16Be,
            true,
            None,
        );
    }
    match std::str::from_utf8(bytes) {
        Ok(s) => (Cow::Borrowed(s), Encoding::Utf8, false, None),
        Err(e) => (
            Cow::Owned(decode_windows_1252(bytes)),
            Encoding::Windows1252,
            false,
            Some(e.valid_up_to()),
        ),
    }
}

fn decode_utf16(bytes: &[u8], big_endian: bool) -> String {
    let units = bytes.chunks_exact(2).map(|pair| {
        if big_endian {
            u16::from_be_bytes([pair[0], pair[1]])
        } else {
            u16::from_le_bytes([pair[0], pair[1]])
        }
    });
    char::decode_utf16(units)
        .map(|c| c.unwrap_or(char::REPLACEMENT_CHARACTER))
        .collect()
}

fn decode_windows_1252(bytes: &[u8]) -> String {
    const HIGH: [char; 32] = [
        '€', '\u{81}', '‚', 'ƒ', '„', '…', '†', '‡', 'ˆ', '‰', 'Š', '‹', 'Œ', '\u{8D}', 'Ž',
        '\u{8F}', '\u{90}', '‘', '’', '“', '”', '•', '–', '—', '˜', '™', 'š', '›', 'œ', '\u{9D}',
        'ž', 'Ÿ',
    ];
    bytes
        .iter()
        .map(|&b| match b {
            0x80..=0x9F => HIGH[(b - 0x80) as usize],
            _ => b as char,
        })
        .collect()
}

/// Guesses the delimiter by picking the candidate that splits the first lines
/// into the most consistent number of fields.
pub fn sniff_delimiter(text: &str) -> u8 {
    const CANDIDATES: [u8; 4] = [b',', b'\t', b';', b'|'];
    let sample = &text.as_bytes()[..text.len().min(64 * 1024)];
    let mut best = (b',', 0.0f64, 0usize);
    for &delim in &CANDIDATES {
        let mut counts: Vec<usize> = Vec::new();
        let mut fields = 1;
        let mut in_quotes = false;
        let mut line_has_content = false;
        for &b in sample {
            match b {
                b'"' => in_quotes = !in_quotes,
                b'\n' | b'\r' if !in_quotes => {
                    if line_has_content {
                        counts.push(fields);
                    }
                    fields = 1;
                    line_has_content = false;
                    if counts.len() >= 100 {
                        break;
                    }
                    continue;
                }
                _ if b == delim && !in_quotes => fields += 1,
                _ => {}
            }
            line_has_content = true;
        }
        if line_has_content && counts.len() < 100 {
            counts.push(fields);
        }
        if counts.is_empty() {
            continue;
        }
        let mut freq: std::collections::HashMap<usize, usize> = Default::default();
        for &c in &counts {
            *freq.entry(c).or_default() += 1;
        }
        let (&mode, &mode_count) = freq.iter().max_by_key(|&(&count, &n)| (n, count)).unwrap();
        if mode < 2 {
            continue;
        }
        let consistency = mode_count as f64 / counts.len() as f64;
        if consistency > best.1 + 1e-9 || ((consistency - best.1).abs() < 1e-9 && mode > best.2) {
            best = (delim, consistency, mode);
        }
    }
    best.0
}

struct Issues {
    list: Vec<Issue>,
    count: usize,
}

impl Issues {
    fn push(&mut self, issue: Issue) {
        self.count += 1;
        if self.list.len() < MAX_ISSUES {
            self.list.push(issue);
        }
    }
}

pub fn parse_str(text: &str, delim: u8) -> Parsed {
    let b = text.as_bytes();
    let n = b.len();
    let mut builder = StoreBuilder::with_capacity(n);
    let mut issues = Issues {
        list: Vec::new(),
        count: 0,
    };
    let mut headers: Option<Vec<String>> = None;
    let mut line_ending: Option<LineEnding> = None;
    let mut pos = 0usize;
    let mut line: u32 = 1;

    while pos < n {
        // Blank lines carry no data.
        if b[pos] == b'\n' || b[pos] == b'\r' {
            if b[pos] == b'\r' && pos + 1 < n && b[pos + 1] == b'\n' {
                pos += 1;
            }
            pos += 1;
            line += 1;
            continue;
        }

        let record_line = line;
        let row = if headers.is_some() {
            Some(builder.rows())
        } else {
            None
        };
        let mut field_index: u32 = 0;
        let mut multiline_from: Option<u32> = None;

        loop {
            builder.begin_field();
            let mut plain = pos >= n || b[pos] != b'"';
            let mut suppress_stray = false;

            if !plain {
                let quote_pos = pos;
                let quote_line = line;
                let arena_start = builder.arena_len();
                pos += 1;
                let mut closed = false;
                while let Some(off) = memchr(b'"', &b[pos..]) {
                    let chunk = &text[pos..pos + off];
                    line += memchr::memchr_iter(b'\n', chunk.as_bytes()).count() as u32;
                    builder.push_str(chunk);
                    pos += off + 1;
                    if pos < n && b[pos] == b'"' {
                        builder.push_char('"');
                        pos += 1;
                    } else {
                        closed = true;
                        break;
                    }
                }
                if closed {
                    if line > quote_line && multiline_from.is_none() {
                        multiline_from = Some(quote_line);
                    }
                    if pos < n && b[pos] != delim && b[pos] != b'\n' && b[pos] != b'\r' {
                        issues.push(Issue {
                            line,
                            row,
                            field: Some(field_index),
                            kind: IssueKind::TextAfterClosingQuote,
                        });
                        let end = memchr3(delim, b'\n', b'\r', &b[pos..])
                            .map(|o| pos + o)
                            .unwrap_or(n);
                        builder.push_str(&text[pos..end]);
                        pos = end;
                    }
                } else {
                    // Recover by treating the quote as a literal character and
                    // reading the rest of its line as an unquoted field.
                    issues.push(Issue {
                        line: quote_line,
                        row,
                        field: Some(field_index),
                        kind: IssueKind::UnterminatedQuote,
                    });
                    builder.rewind_row(field_index as usize + 1, arena_start);
                    pos = quote_pos;
                    line = quote_line;
                    plain = true;
                    suppress_stray = true;
                }
            }

            if plain {
                let end = memchr3(delim, b'\n', b'\r', &b[pos..])
                    .map(|o| pos + o)
                    .unwrap_or(n);
                let field = &text[pos..end];
                if !suppress_stray && memchr(b'"', field.as_bytes()).is_some() {
                    issues.push(Issue {
                        line,
                        row,
                        field: Some(field_index),
                        kind: IssueKind::StrayQuote,
                    });
                }
                builder.push_str(field);
                pos = end;
            }

            field_index += 1;
            if pos >= n {
                break;
            }
            if b[pos] == delim {
                pos += 1;
                continue;
            }
            // Record terminator.
            let ending = if b[pos] == b'\r' {
                if pos + 1 < n && b[pos + 1] == b'\n' {
                    pos += 2;
                    LineEnding::CrLf
                } else {
                    pos += 1;
                    LineEnding::Cr
                }
            } else {
                pos += 1;
                LineEnding::Lf
            };
            line_ending.get_or_insert(ending);
            line += 1;
            break;
        }

        match &headers {
            None => {
                let names = builder.take_row();
                let mut seen = std::collections::HashSet::new();
                for name in &names {
                    if !name.is_empty() && !seen.insert(name.as_str()) {
                        issues.push(Issue {
                            line: record_line,
                            row: None,
                            field: None,
                            kind: IssueKind::DuplicateHeader { name: name.clone() },
                        });
                    }
                }
                headers = Some(names);
            }
            Some(names) => {
                let found = field_index as usize;
                if found != names.len() {
                    issues.push(Issue {
                        line: record_line,
                        row,
                        field: None,
                        kind: IssueKind::FieldCount {
                            expected: names.len(),
                            found,
                            multiline_from,
                        },
                    });
                }
                builder.end_row(found);
            }
        }
    }

    let mut headers = headers.unwrap_or_default();
    let store = builder.finish(headers.len());
    for i in headers.len()..store.total_cols() as usize {
        headers.push(format!("Column {}", i + 1));
    }
    Parsed {
        headers,
        store,
        delimiter: delim,
        line_ending: line_ending.unwrap_or(LineEnding::Lf),
        encoding: Encoding::Utf8,
        has_bom: false,
        issues: issues.list,
        issue_count: issues.count,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows(p: &Parsed) -> Vec<Vec<String>> {
        (0..p.store.total_rows())
            .map(|r| {
                (0..p.store.total_cols())
                    .map(|c| p.store.get(r, c).to_string())
                    .collect()
            })
            .collect()
    }

    #[test]
    fn simple() {
        let p = parse_str("a,b,c\n1,2,3\n4,5,6\n", b',');
        assert_eq!(p.headers, vec!["a", "b", "c"]);
        assert_eq!(rows(&p), vec![vec!["1", "2", "3"], vec!["4", "5", "6"]]);
        assert!(p.issues.is_empty());
        assert_eq!(p.line_ending, LineEnding::Lf);
    }

    #[test]
    fn quoted_fields_and_crlf() {
        let p = parse_str(
            "name,note\r\n\"Smith, J\",\"said \"\"hi\"\"\"\r\nx,\"multi\r\nline\"\r\n",
            b',',
        );
        assert_eq!(
            rows(&p),
            vec![vec!["Smith, J", "said \"hi\""], vec!["x", "multi\r\nline"]]
        );
        assert!(p.issues.is_empty(), "{:?}", p.issues);
        assert_eq!(p.line_ending, LineEnding::CrLf);
    }

    #[test]
    fn empty_fields_and_trailing_delimiter() {
        let p = parse_str("a,b,c\n,,\n1,,3", b',');
        assert_eq!(rows(&p), vec![vec!["", "", ""], vec!["1", "", "3"]]);
        assert!(p.issues.is_empty());
    }

    #[test]
    fn field_count_issues_point_at_lines() {
        let p = parse_str("a,b,c\n1,2,3\n4,5\n6,7,8,9\n", b',');
        assert_eq!(p.issues.len(), 2);
        assert_eq!(p.issues[0].line, 3);
        assert_eq!(p.issues[0].row, Some(1));
        assert!(matches!(
            p.issues[0].kind,
            IssueKind::FieldCount {
                expected: 3,
                found: 2,
                ..
            }
        ));
        assert_eq!(p.issues[1].line, 4);
        assert_eq!(p.headers.len(), 4);
        assert_eq!(p.headers[3], "Column 4");
        assert_eq!(rows(&p)[2], vec!["6", "7", "8", "9"]);
    }

    #[test]
    fn unterminated_quote_recovers_at_line_end() {
        let p = parse_str("a,b\n1,\"oops\n3,4\n", b',');
        assert_eq!(p.issues[0].kind, IssueKind::UnterminatedQuote);
        assert_eq!(p.issues[0].line, 2);
        assert_eq!(p.issues[0].field, Some(1));
        assert_eq!(rows(&p), vec![vec!["1", "\"oops"], vec!["3", "4"]]);
    }

    #[test]
    fn stray_and_trailing_quotes() {
        let p = parse_str("a,b\n1,ab\"c\n\"x\"y,2\n", b',');
        assert_eq!(p.issues[0].kind, IssueKind::StrayQuote);
        assert_eq!(p.issues[0].line, 2);
        assert_eq!(p.issues[1].kind, IssueKind::TextAfterClosingQuote);
        assert_eq!(p.issues[1].line, 3);
        assert_eq!(rows(&p), vec![vec!["1", "ab\"c"], vec!["xy", "2"]]);
    }

    #[test]
    fn multiline_hint_on_unbalanced_quote() {
        let p = parse_str("a,b\n\"1,2\n3,4\n5,\"6\n", b',');
        let count = p
            .issues
            .iter()
            .find(|i| matches!(i.kind, IssueKind::FieldCount { .. }))
            .unwrap();
        assert_eq!(
            count.kind,
            IssueKind::FieldCount {
                expected: 2,
                found: 1,
                multiline_from: Some(2)
            }
        );
        assert!(count.to_string().contains("line 2"));
    }

    #[test]
    fn duplicate_headers_are_warnings() {
        let p = parse_str("a,a\n1,2\n", b',');
        assert_eq!(p.issues.len(), 1);
        assert!(!p.issues[0].is_error());
    }

    #[test]
    fn sniffs_delimiters() {
        assert_eq!(sniff_delimiter("a;b;c\n1;2;3\n"), b';');
        assert_eq!(sniff_delimiter("a\tb\n1\t2\n"), b'\t');
        assert_eq!(sniff_delimiter("a,b\n\"1;2\",3\n"), b',');
        assert_eq!(sniff_delimiter("a|b|c\n1|2|3"), b'|');
        assert_eq!(sniff_delimiter("single\nvalue\n"), b',');
    }

    #[test]
    fn decodes_bom_and_latin1() {
        let p = parse_bytes(b"\xEF\xBB\xBFa,b\n1,2\n", None);
        assert!(p.has_bom);
        assert_eq!(p.headers, vec!["a", "b"]);
        let p = parse_bytes(b"name\ncaf\xE9\n", None);
        assert_eq!(p.encoding, Encoding::Windows1252);
        assert_eq!(p.store.get(0, 0), "café");
        assert_eq!(p.issues[0].line, 2);
    }

    #[test]
    fn empty_input() {
        let p = parse_str("", b',');
        assert!(p.headers.is_empty());
        assert_eq!(p.store.total_rows(), 0);
    }
}

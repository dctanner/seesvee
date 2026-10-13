//! Interpreting cell text: numbers and natural ("alphanumeric") ordering.

use std::cmp::Ordering;

/// Parses a cell as a number, tolerating surrounding whitespace, thousands
/// separators, a leading currency sign and a trailing percent sign.
pub fn parse_number(s: &str) -> Option<f64> {
    let s = s.trim();
    let first = *s.as_bytes().first()?;
    if !(first.is_ascii_digit()
        || matches!(first, b'-' | b'+' | b'.' | b'$')
        || s.starts_with('€')
        || s.starts_with('£'))
    {
        return None;
    }
    if let Ok(n) = s.parse::<f64>() {
        return n.is_finite().then_some(n);
    }
    let (negative, s) = match s.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, s.strip_prefix('+').unwrap_or(s)),
    };
    let s = s
        .strip_prefix('$')
        .or_else(|| s.strip_prefix('€'))
        .or_else(|| s.strip_prefix('£'))
        .unwrap_or(s);
    let (s, percent) = match s.strip_suffix('%') {
        Some(rest) => (rest, true),
        None => (s, false),
    };
    let cleaned: String = if s.contains(',') {
        // Only accept well-formed thousands grouping such as 1,234,567.89.
        let int_part = s.split('.').next().unwrap_or("");
        let groups: Vec<&str> = int_part.split(',').collect();
        let valid = !groups[0].is_empty()
            && groups[0].len() <= 3
            && groups[1..].iter().all(|g| g.len() == 3);
        if !valid {
            return None;
        }
        s.replace(',', "")
    } else {
        s.to_string()
    };
    if cleaned.is_empty()
        || !cleaned.bytes().all(|b| {
            b.is_ascii_digit() || b == b'.' || b == b'e' || b == b'E' || b == b'-' || b == b'+'
        })
    {
        return None;
    }
    let mut n = cleaned.parse::<f64>().ok()?;
    if percent {
        n /= 100.0;
    }
    if negative {
        n = -n;
    }
    n.is_finite().then_some(n)
}

/// Compares strings case-insensitively, treating runs of digits as numbers so
/// that "file2" sorts before "file10".
pub fn natural_cmp(a: &str, b: &str) -> Ordering {
    let (ab, bb) = (a.as_bytes(), b.as_bytes());
    let (mut i, mut j) = (0, 0);
    while i < ab.len() && j < bb.len() {
        let (ca, cb) = (ab[i], bb[j]);
        if ca.is_ascii_digit() && cb.is_ascii_digit() {
            let si = i;
            while i < ab.len() && ab[i].is_ascii_digit() {
                i += 1;
            }
            let sj = j;
            while j < bb.len() && bb[j].is_ascii_digit() {
                j += 1;
            }
            let na = trim_zeros(&ab[si..i]);
            let nb = trim_zeros(&bb[sj..j]);
            let ord = na.len().cmp(&nb.len()).then_with(|| na.cmp(nb));
            if ord != Ordering::Equal {
                return ord;
            }
            continue;
        }
        if ca.is_ascii() && cb.is_ascii() {
            let ord = ca.to_ascii_lowercase().cmp(&cb.to_ascii_lowercase());
            if ord != Ordering::Equal {
                return ord;
            }
            i += 1;
            j += 1;
            continue;
        }
        // Non-ASCII: compare the remaining text by lowercased chars.
        let ra = a[i..].chars().flat_map(char::to_lowercase);
        let rb = b[j..].chars().flat_map(char::to_lowercase);
        return ra.cmp(rb).then_with(|| a.cmp(b));
    }
    (ab.len() - i).cmp(&(bb.len() - j)).then_with(|| a.cmp(b))
}

fn trim_zeros(digits: &[u8]) -> &[u8] {
    let first = digits
        .iter()
        .position(|&d| d != b'0')
        .unwrap_or(digits.len());
    &digits[first..]
}

/// A precomputed sort key for one cell.
#[derive(Clone, Copy)]
pub enum SortKey<'a> {
    Number(f64),
    Text(&'a str),
    Empty,
}

impl<'a> SortKey<'a> {
    pub fn new(s: &'a str) -> Self {
        if s.trim().is_empty() {
            SortKey::Empty
        } else if let Some(n) = parse_number(s) {
            SortKey::Number(n)
        } else {
            SortKey::Text(s)
        }
    }

    /// Ascending order: numbers, then text, with empty cells always last
    /// (the caller keeps them last when reversing).
    pub fn cmp_asc(&self, other: &Self) -> Ordering {
        use SortKey::*;
        match (self, other) {
            (Number(a), Number(b)) => a.total_cmp(b),
            (Number(_), Text(_)) => Ordering::Less,
            (Text(_), Number(_)) => Ordering::Greater,
            (Text(a), Text(b)) => natural_cmp(a, b),
            (Empty, Empty) => Ordering::Equal,
            (Empty, _) => Ordering::Greater,
            (_, Empty) => Ordering::Less,
        }
    }

    pub fn is_empty(&self) -> bool {
        matches!(self, SortKey::Empty)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers() {
        assert_eq!(parse_number("42"), Some(42.0));
        assert_eq!(parse_number(" -3.5 "), Some(-3.5));
        assert_eq!(parse_number("1,234,567.5"), Some(1234567.5));
        assert_eq!(parse_number("$1,200"), Some(1200.0));
        assert_eq!(parse_number("-$5"), Some(-5.0));
        assert_eq!(parse_number("50%"), Some(0.5));
        assert_eq!(parse_number("1e3"), Some(1000.0));
        assert_eq!(parse_number("12,34"), None);
        assert_eq!(parse_number("abc"), None);
        assert_eq!(parse_number("12abc"), None);
        assert_eq!(parse_number("inf"), None);
        assert_eq!(parse_number("NaN"), None);
        assert_eq!(parse_number(""), None);
    }

    #[test]
    fn natural() {
        let mut v = vec![
            "file10", "File2", "file1", "apple", "Banana", "file02b", "é",
        ];
        v.sort_by(|a, b| natural_cmp(a, b));
        assert_eq!(
            v,
            vec![
                "apple", "Banana", "file1", "File2", "file02b", "file10", "é"
            ]
        );
    }

    #[test]
    fn keys() {
        let mut v = vec!["b", "", "10", "a", "9", " "];
        v.sort_by(|a, b| SortKey::new(a).cmp_asc(&SortKey::new(b)));
        assert_eq!(v, vec!["9", "10", "a", "b", "", " "]);
    }
}

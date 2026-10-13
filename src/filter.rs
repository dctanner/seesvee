//! Column filters: a condition plus an optional set of chosen values,
//! modelled on the spreadsheet autofilter.

use std::collections::HashSet;

use crate::value::parse_number;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FilterOp {
    Contains,
    NotContains,
    Equals,
    NotEquals,
    BeginsWith,
    EndsWith,
    IsEmpty,
    IsNotEmpty,
    Greater,
    GreaterOrEqual,
    Less,
    LessOrEqual,
    Between,
    NotBetween,
}

impl FilterOp {
    pub const ALL: [FilterOp; 14] = [
        FilterOp::Contains,
        FilterOp::NotContains,
        FilterOp::Equals,
        FilterOp::NotEquals,
        FilterOp::BeginsWith,
        FilterOp::EndsWith,
        FilterOp::IsEmpty,
        FilterOp::IsNotEmpty,
        FilterOp::Greater,
        FilterOp::GreaterOrEqual,
        FilterOp::Less,
        FilterOp::LessOrEqual,
        FilterOp::Between,
        FilterOp::NotBetween,
    ];

    pub fn label(self) -> &'static str {
        match self {
            FilterOp::Contains => "Contains",
            FilterOp::NotContains => "Does not contain",
            FilterOp::Equals => "Equals",
            FilterOp::NotEquals => "Does not equal",
            FilterOp::BeginsWith => "Begins with",
            FilterOp::EndsWith => "Ends with",
            FilterOp::IsEmpty => "Is empty",
            FilterOp::IsNotEmpty => "Is not empty",
            FilterOp::Greater => "Greater than",
            FilterOp::GreaterOrEqual => "Greater than or equal to",
            FilterOp::Less => "Less than",
            FilterOp::LessOrEqual => "Less than or equal to",
            FilterOp::Between => "Between",
            FilterOp::NotBetween => "Not between",
        }
    }

    /// How many operand inputs the condition needs.
    pub fn operands(self) -> usize {
        match self {
            FilterOp::IsEmpty | FilterOp::IsNotEmpty => 0,
            FilterOp::Between | FilterOp::NotBetween => 2,
            _ => 1,
        }
    }

    /// Whether this group starts a new section in the condition menu.
    pub fn starts_group(self) -> bool {
        matches!(
            self,
            FilterOp::Equals | FilterOp::IsEmpty | FilterOp::Greater | FilterOp::Between
        )
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Condition {
    pub op: Option<FilterOp>,
    pub value: String,
    pub value2: String,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ValueSet {
    /// Only these values pass.
    Only(HashSet<String>),
    /// Every value except these passes.
    Except(HashSet<String>),
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Filter {
    pub condition: Condition,
    pub values: Option<ValueSet>,
}

impl Filter {
    pub fn is_active(&self) -> bool {
        self.condition.op.is_some() || self.values.is_some()
    }

    pub fn compile(&self) -> CompiledFilter<'_> {
        let op = self.condition.op;
        CompiledFilter {
            op,
            needle: self.condition.value.to_lowercase(),
            needle2: self.condition.value2.to_lowercase(),
            n1: parse_number(&self.condition.value),
            n2: parse_number(&self.condition.value2),
            values: self.values.as_ref(),
        }
    }
}

/// A filter with its operands pre-processed for fast matching.
pub struct CompiledFilter<'a> {
    op: Option<FilterOp>,
    needle: String,
    needle2: String,
    n1: Option<f64>,
    n2: Option<f64>,
    values: Option<&'a ValueSet>,
}

impl CompiledFilter<'_> {
    pub fn matches(&self, cell: &str) -> bool {
        if let Some(values) = self.values {
            let pass = match values {
                ValueSet::Only(set) => set.contains(cell),
                ValueSet::Except(set) => !set.contains(cell),
            };
            if !pass {
                return false;
            }
        }
        let Some(op) = self.op else {
            return true;
        };
        match op {
            FilterOp::IsEmpty => cell.trim().is_empty(),
            FilterOp::IsNotEmpty => !cell.trim().is_empty(),
            FilterOp::Contains => lower(cell).contains(&self.needle),
            FilterOp::NotContains => !lower(cell).contains(&self.needle),
            FilterOp::BeginsWith => lower(cell).starts_with(&self.needle),
            FilterOp::EndsWith => lower(cell).ends_with(&self.needle),
            FilterOp::Equals | FilterOp::NotEquals => {
                let equal = match (self.n1, parse_number(cell)) {
                    (Some(a), Some(b)) => a == b,
                    _ => lower(cell).trim() == self.needle.trim(),
                };
                equal == (op == FilterOp::Equals)
            }
            FilterOp::Greater
            | FilterOp::GreaterOrEqual
            | FilterOp::Less
            | FilterOp::LessOrEqual => {
                let ord = match (parse_number(cell), self.n1) {
                    (Some(a), Some(b)) => a.partial_cmp(&b),
                    (None, None) if !cell.trim().is_empty() => {
                        Some(crate::value::natural_cmp(&lower(cell), &self.needle))
                    }
                    _ => None,
                };
                let Some(ord) = ord else { return false };
                match op {
                    FilterOp::Greater => ord.is_gt(),
                    FilterOp::GreaterOrEqual => ord.is_ge(),
                    FilterOp::Less => ord.is_lt(),
                    _ => ord.is_le(),
                }
            }
            FilterOp::Between | FilterOp::NotBetween => {
                let inside = match (parse_number(cell), self.n1, self.n2) {
                    (Some(x), Some(a), Some(b)) => x >= a.min(b) && x <= a.max(b),
                    (None, None, None) if !cell.trim().is_empty() => {
                        let x = lower(cell);
                        let (lo, hi) =
                            if crate::value::natural_cmp(&self.needle, &self.needle2).is_le() {
                                (&self.needle, &self.needle2)
                            } else {
                                (&self.needle2, &self.needle)
                            };
                        crate::value::natural_cmp(&x, lo).is_ge()
                            && crate::value::natural_cmp(&x, hi).is_le()
                    }
                    _ => return op == FilterOp::NotBetween && !cell.trim().is_empty(),
                };
                inside == (op == FilterOp::Between)
            }
        }
    }
}

fn lower(s: &str) -> std::borrow::Cow<'_, str> {
    if s.bytes().any(|b| b.is_ascii_uppercase() || !b.is_ascii()) {
        std::borrow::Cow::Owned(s.to_lowercase())
    } else {
        std::borrow::Cow::Borrowed(s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cond(op: FilterOp, v: &str, v2: &str) -> Filter {
        Filter {
            condition: Condition {
                op: Some(op),
                value: v.into(),
                value2: v2.into(),
            },
            values: None,
        }
    }

    #[test]
    fn text_conditions() {
        let f = cond(FilterOp::Contains, "OO", "");
        assert!(f.compile().matches("Foobar"));
        assert!(!f.compile().matches("bar"));
        assert!(
            cond(FilterOp::BeginsWith, "fo", "")
                .compile()
                .matches("Foo")
        );
        assert!(cond(FilterOp::EndsWith, "AR", "").compile().matches("bar"));
        assert!(
            cond(FilterOp::NotContains, "x", "")
                .compile()
                .matches("abc")
        );
        assert!(cond(FilterOp::IsEmpty, "", "").compile().matches("  "));
        assert!(cond(FilterOp::IsNotEmpty, "", "").compile().matches("a"));
        assert!(cond(FilterOp::Equals, "abc", "").compile().matches("ABC"));
        assert!(
            cond(FilterOp::NotEquals, "abc", "")
                .compile()
                .matches("abcd")
        );
    }

    #[test]
    fn numeric_conditions() {
        assert!(cond(FilterOp::Equals, "5", "").compile().matches("5.0"));
        assert!(cond(FilterOp::Greater, "10", "").compile().matches("10.5"));
        assert!(!cond(FilterOp::Greater, "10", "").compile().matches("9"));
        assert!(!cond(FilterOp::Greater, "10", "").compile().matches("abc"));
        assert!(
            cond(FilterOp::LessOrEqual, "10", "")
                .compile()
                .matches("10")
        );
        assert!(cond(FilterOp::Between, "10", "1").compile().matches("5"));
        assert!(!cond(FilterOp::Between, "1", "10").compile().matches("11"));
        assert!(
            cond(FilterOp::NotBetween, "1", "10")
                .compile()
                .matches("11")
        );
        assert!(cond(FilterOp::Greater, "b", "").compile().matches("c"));
    }

    #[test]
    fn value_sets() {
        let mut f = Filter::default();
        f.values = Some(ValueSet::Only(["a".to_string()].into()));
        assert!(f.compile().matches("a"));
        assert!(!f.compile().matches("b"));
        f.values = Some(ValueSet::Except(["a".to_string()].into()));
        assert!(!f.compile().matches("a"));
        assert!(f.compile().matches("b"));
    }
}

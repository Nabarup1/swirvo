//! Tool options: what a tool can ask for (a password, watermark text, a paper size), as
//! data. The Board draws a form from it, the CLI takes `--set key=value`, and the wheel
//! runs tools whose options all have defaults straight away.

use std::collections::BTreeMap;

/// The values given for a tool's fields, by key.
pub type ToolArgs = BTreeMap<String, String>;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum FieldKind {
    Text,
    /// Shown as dots; never logged.
    Secret,
    Number {
        min: f64,
        max: f64,
    },
    /// (value, label) pairs; the value is what the tool receives.
    Choice(&'static [(&'static str, &'static str)]),
    /// "true" / "false".
    Flag,
    /// A file path (an image for a watermark or signature).
    File,
    /// Pages of the PDF: `1-3, 7, 10-`, `odd` (checked against the file when it's known).
    Pages,
    /// Several page selections, one per output file: `1-15, 16; 25-40, 42`.
    PageGroups,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Field {
    pub key: &'static str,
    pub label: &'static str,
    pub kind: FieldKind,
    /// Used when the user gives nothing. Empty + `required` means the tool must ask.
    pub default: &'static str,
    pub required: bool,
    /// One line under the field.
    pub hint: &'static str,
}

impl Field {
    pub const fn text(key: &'static str, label: &'static str, default: &'static str) -> Field {
        Field {
            key,
            label,
            kind: FieldKind::Text,
            default,
            required: false,
            hint: "",
        }
    }

    pub const fn required(mut self) -> Field {
        self.required = true;
        self
    }

    pub const fn hint(mut self, h: &'static str) -> Field {
        self.hint = h;
        self
    }

    pub const fn secret(key: &'static str, label: &'static str) -> Field {
        Field {
            key,
            label,
            kind: FieldKind::Secret,
            default: "",
            required: false,
            hint: "",
        }
    }

    pub const fn number(
        key: &'static str,
        label: &'static str,
        default: &'static str,
        min: f64,
        max: f64,
    ) -> Field {
        Field {
            key,
            label,
            kind: FieldKind::Number { min, max },
            default,
            required: false,
            hint: "",
        }
    }

    pub const fn choice(
        key: &'static str,
        label: &'static str,
        options: &'static [(&'static str, &'static str)],
        default: &'static str,
    ) -> Field {
        Field {
            key,
            label,
            kind: FieldKind::Choice(options),
            default,
            required: false,
            hint: "",
        }
    }

    pub const fn flag(key: &'static str, label: &'static str, default: bool) -> Field {
        Field {
            key,
            label,
            kind: FieldKind::Flag,
            default: if default { "true" } else { "false" },
            required: false,
            hint: "",
        }
    }

    pub const fn pages(key: &'static str, label: &'static str) -> Field {
        Field {
            key,
            label,
            kind: FieldKind::Pages,
            default: "",
            required: false,
            hint: "",
        }
    }

    pub const fn page_groups(key: &'static str, label: &'static str) -> Field {
        Field {
            key,
            label,
            kind: FieldKind::PageGroups,
            default: "",
            required: false,
            hint: "",
        }
    }

    pub const fn file(key: &'static str, label: &'static str) -> Field {
        Field {
            key,
            label,
            kind: FieldKind::File,
            default: "",
            required: false,
            hint: "",
        }
    }

    /// Check one value; the message is for the user (docs/17 §3.1: what's wrong, in
    /// plain words, and what to type instead).
    pub fn validate(&self, v: &str) -> Result<(), String> {
        if v.trim().is_empty() {
            return if self.required {
                Err(format!("{} is needed", self.label))
            } else {
                Ok(())
            };
        }
        match self.kind {
            FieldKind::Number { min, max } => match v.trim().replace(',', ".").parse::<f64>() {
                Ok(n) if n >= min && n <= max => Ok(()),
                Ok(_) => Err(format!("{}: between {min} and {max}", self.label)),
                Err(_) => Err(format!("{}: type a number", self.label)),
            },
            FieldKind::Choice(opts) => {
                if opts
                    .iter()
                    .any(|(val, _)| val.eq_ignore_ascii_case(v.trim()))
                {
                    Ok(())
                } else {
                    let all: Vec<&str> = opts.iter().map(|o| o.0).collect();
                    Err(format!("{}: one of {}", self.label, all.join(", ")))
                }
            }
            FieldKind::Flag => match v.trim().to_ascii_lowercase().as_str() {
                "true" | "false" | "yes" | "no" | "1" | "0" | "on" | "off" => Ok(()),
                _ => Err(format!("{}: yes or no", self.label)),
            },
            FieldKind::File => {
                if std::path::Path::new(v.trim()).is_file() {
                    Ok(())
                } else {
                    Err(format!("{}: file not found", self.label))
                }
            }
            FieldKind::Pages => crate::pages::check(v).map_err(|e| format!("{}: {e}", self.label)),
            FieldKind::PageGroups => crate::pages::check_groups(v),
            FieldKind::Text | FieldKind::Secret => Ok(()),
        }
    }

    /// [`Field::validate`], and page numbers checked against a PDF of `pages` pages.
    pub fn validate_for(&self, v: &str, pages: Option<usize>) -> Result<(), String> {
        self.validate(v)?;
        match (self.kind, pages) {
            (FieldKind::Pages, Some(n)) if !v.trim().is_empty() => crate::pages::parse(v, n)
                .map(|_| ())
                .map_err(|e| format!("{}: {e}", self.label)),
            (FieldKind::PageGroups, Some(n)) if !v.trim().is_empty() => {
                crate::pages::parse_groups(v, n).map(|_| ())
            }
            _ => Ok(()),
        }
    }
}

/// Read typed values out of [`ToolArgs`] with the fields' defaults.
pub struct Values<'a> {
    fields: &'static [Field],
    args: &'a ToolArgs,
}

impl<'a> Values<'a> {
    /// Validate `args` against `fields`. Unknown keys are an error so typos don't pass
    /// silently on the command line.
    pub fn new(fields: &'static [Field], args: &'a ToolArgs) -> Result<Values<'a>, String> {
        for k in args.keys() {
            if !fields.iter().any(|f| f.key == k) {
                let known: Vec<&str> = fields.iter().map(|f| f.key).collect();
                return Err(if known.is_empty() {
                    format!("this tool has no option “{k}”")
                } else {
                    format!("unknown option “{k}” (options: {})", known.join(", "))
                });
            }
        }
        for f in fields {
            let v = args.get(f.key).map_or(f.default, String::as_str);
            f.validate(v)?;
        }
        Ok(Values { fields, args })
    }

    pub fn str(&self, key: &str) -> &str {
        match self.args.get(key) {
            Some(v) => v.as_str(),
            None => self
                .fields
                .iter()
                .find(|f| f.key == key)
                .map_or("", |f| f.default),
        }
    }

    pub fn opt(&self, key: &str) -> Option<&str> {
        let v = self.str(key).trim();
        (!v.is_empty()).then_some(v)
    }

    pub fn num(&self, key: &str) -> Option<f64> {
        self.opt(key)?.replace(',', ".").parse().ok()
    }

    pub fn flag(&self, key: &str) -> bool {
        matches!(
            self.str(key).trim().to_ascii_lowercase().as_str(),
            "true" | "yes" | "1" | "on"
        )
    }
}

/// Parse `key=value` pairs (the CLI's `--set`).
pub fn parse_pairs(pairs: &[String]) -> Result<ToolArgs, String> {
    let mut a = ToolArgs::new();
    for p in pairs {
        let (k, v) = p
            .split_once('=')
            .ok_or_else(|| format!("“{p}”: write it as key=value"))?;
        a.insert(k.trim().to_string(), v.to_string());
    }
    Ok(a)
}

#[cfg(test)]
mod tests {
    use super::*;

    static FIELDS: &[Field] = &[
        Field::text("text", "Text", "").required(),
        Field::number("n", "Pages", "2", 1.0, 100.0),
        Field::choice(
            "paper",
            "Paper",
            &[("a4", "A4"), ("letter", "Letter")],
            "a4",
        ),
        Field::flag("copy", "Allow copying", true),
    ];

    #[test]
    fn defaults_validation_and_typos() {
        let mut a = ToolArgs::new();
        assert!(Values::new(FIELDS, &a).is_err(), "text is required");
        a.insert("text".into(), "DRAFT".into());
        let v = Values::new(FIELDS, &a).unwrap();
        assert_eq!(v.num("n"), Some(2.0));
        assert_eq!(v.str("paper"), "a4");
        assert!(v.flag("copy"));
        a.insert("n".into(), "500".into());
        assert!(Values::new(FIELDS, &a).is_err());
        a.insert("n".into(), "3".into());
        a.insert("papper".into(), "a4".into());
        let e = Values::new(FIELDS, &a).err().unwrap();
        assert!(e.contains("papper") && e.contains("paper"), "{e}");
        assert_eq!(
            parse_pairs(&["a=1".into(), "b= x=y".into()]).unwrap()["b"],
            " x=y"
        );
        assert!(parse_pairs(&["nope".into()]).is_err());
    }
}

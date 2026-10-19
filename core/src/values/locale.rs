//! Explicit locale presets: a decimal convention plus a day/month order, usable as the hint for
//! one column ([`Locale::hints`]) or a whole sheet ([`super::compute_with`]). Without a preset
//! everything is inferred from the data (see [`super::sheet_column_hints`]).

use super::date::DateOrder;
use super::features::ColumnHints;
use super::number::{DecimalSep, NumberHint};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Locale {
    /// United States: `1,234.56`, `12/31/2024`.
    Us,
    /// United Kingdom / Ireland: `1,234.56`, `31/12/2024`.
    Uk,
    /// Germany / Austria: `1.234,56`, `31.12.2024`.
    De,
    /// France / Belgium: `1 234,56` (narrow NBSP), `31/12/2024`.
    Fr,
    /// Spain: `1.234,56`, `31/12/2024`.
    Es,
    /// Italy: `1.234,56`, `31/12/2024`.
    It,
    /// Netherlands: `1.234,56`, `31-12-2024`.
    Nl,
    /// Portugal / Brazil: `1.234,56` / `1 234,56`, `31/12/2024`.
    Pt,
    /// Poland: `1 234,56`, `31.12.2024`.
    Pl,
    /// Switzerland: `1'234.56`, `31.12.2024`.
    Ch,
    /// Serbia and ex-Yugoslav countries: `1.234,56`, `31.12.2024.`.
    Rs,
    /// ISO / machine output: `1234.56`, `2024-12-31` (no day/month order preference).
    Iso,
}

impl Locale {
    pub const ALL: [Locale; 12] = [
        Locale::Us,
        Locale::Uk,
        Locale::De,
        Locale::Fr,
        Locale::Es,
        Locale::It,
        Locale::Nl,
        Locale::Pt,
        Locale::Pl,
        Locale::Ch,
        Locale::Rs,
        Locale::Iso,
    ];

    pub fn decimal(self) -> DecimalSep {
        match self {
            Locale::Us | Locale::Uk | Locale::Ch | Locale::Iso => DecimalSep::Dot,
            _ => DecimalSep::Comma,
        }
    }

    pub fn date_order(self) -> Option<DateOrder> {
        match self {
            Locale::Us => Some(DateOrder::MonthDay),
            Locale::Iso => None,
            _ => Some(DateOrder::DayMonth),
        }
    }

    pub fn hints(self) -> ColumnHints {
        ColumnHints { decimal: Some(self.decimal()), date_order: self.date_order() }
    }

    pub fn number_hint(self) -> NumberHint {
        NumberHint::with_decimal(Some(self.decimal()))
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Locale::Us => "us",
            Locale::Uk => "uk",
            Locale::De => "de",
            Locale::Fr => "fr",
            Locale::Es => "es",
            Locale::It => "it",
            Locale::Nl => "nl",
            Locale::Pt => "pt",
            Locale::Pl => "pl",
            Locale::Ch => "ch",
            Locale::Rs => "rs",
            Locale::Iso => "iso",
        }
    }

    /// Parses a preset name or a BCP-47-ish tag: `us`, `en-US`, `en_GB`, `de`, `de-AT`,
    /// `fr-CH` / `de-CH` (→ Ch), `sr`, `sr-Latn-RS`, `hr`, `bs`, `pt-BR`, `iso` (case-insensitive).
    pub fn parse(tag: &str) -> Option<Locale> {
        let t = tag.trim().to_ascii_lowercase().replace('_', "-");
        let mut parts = t.split('-');
        let lang = parts.next()?;
        let region = parts.rfind(|p| p.len() == 2);
        if region == Some("ch") || lang == "ch" {
            return Some(Locale::Ch);
        }
        Some(match (lang, region) {
            ("us", _) | ("en", Some("us" | "ph")) | ("en", None) => Locale::Us,
            ("uk" | "gb", _) | ("en", Some(_)) => Locale::Uk,
            ("de", _) => Locale::De,
            ("fr", _) => Locale::Fr,
            ("es", _) => Locale::Es,
            ("it", _) => Locale::It,
            ("nl", _) => Locale::Nl,
            ("pt" | "br", _) => Locale::Pt,
            ("pl", _) => Locale::Pl,
            ("rs" | "sr" | "hr" | "bs" | "me" | "sl" | "mk" | "cnr", _) => Locale::Rs,
            ("iso", _) => Locale::Iso,
            _ => return None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presets() {
        assert_eq!(Locale::Us.hints(), ColumnHints { decimal: Some(DecimalSep::Dot), date_order: Some(DateOrder::MonthDay) });
        assert_eq!(Locale::Uk.date_order(), Some(DateOrder::DayMonth));
        assert_eq!(Locale::Ch.decimal(), DecimalSep::Dot);
        assert_eq!(Locale::De.decimal(), DecimalSep::Comma);
        assert_eq!(Locale::Iso.date_order(), None);
        for l in Locale::ALL {
            assert_eq!(Locale::parse(l.as_str()), Some(l));
        }
    }

    #[test]
    fn tags() {
        assert_eq!(Locale::parse("en-US"), Some(Locale::Us));
        assert_eq!(Locale::parse("en_GB"), Some(Locale::Uk));
        assert_eq!(Locale::parse("en-IE"), Some(Locale::Uk));
        assert_eq!(Locale::parse("de-AT"), Some(Locale::De));
        assert_eq!(Locale::parse("de-CH"), Some(Locale::Ch));
        assert_eq!(Locale::parse("fr-CH"), Some(Locale::Ch));
        assert_eq!(Locale::parse("sr-Latn-RS"), Some(Locale::Rs));
        assert_eq!(Locale::parse("hr"), Some(Locale::Rs));
        assert_eq!(Locale::parse("pt-BR"), Some(Locale::Pt));
        assert_eq!(Locale::parse("xx"), None);
    }
}

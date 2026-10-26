// How big an issue is, on its team's own estimate scale. Points mean a different size on
// each team (Linear lets a team choose exponential, Fibonacci, linear or T-shirt, extended
// or not, with zero or not), so every threshold Tolkster keeps (the deep review, the
// ladder's rungs) is a Size: a position on whatever scale the issue's team
// uses, XS the first and XL the fifth. `tolkster issue --estimate` and a triage verdict
// refuse any value off the team's scale.

use serde::{Deserialize, Serialize, Serializer};

// A query names these as `TEAM_FIELDS`, which `with_team_fields` fills in.
pub const TEAM_FIELDS: &str = "issueEstimationType issueEstimationExtended issueEstimationAllowZero";

// A team's `TEAM_FIELDS`, as Linear answers them: a team read with them flattens this in.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Estimation {
    pub issue_estimation_type: String,
    pub issue_estimation_extended: bool,
    pub issue_estimation_allow_zero: bool,
}

pub fn with_team_fields(query: &str) -> String {
    query.replace("TEAM_FIELDS", TEAM_FIELDS)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    NotUsed,
    Exponential,
    Fibonacci,
    Linear,
    TShirt,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Scale {
    pub kind: Kind,
    pub extended: bool,
    pub zero: bool,
}

// A position on a scale: 0 a zero estimate, 1 XS (the first value), up to 7 XXXL (the
// last of an extended scale).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Size(pub u8);

const NAMES: [&str; 8] = ["zero", "XS", "S", "M", "L", "XL", "XXL", "XXXL"];

// The points Tolkster's thresholds were once set in: the T-shirt scale's, extended.
const T_SHIRT: [u32; 7] = [1, 2, 3, 5, 8, 13, 21];

impl Size {
    pub const XS: Size = Size(1);
    pub const S: Size = Size(2);
    pub const M: Size = Size(3);
    #[cfg(test)]
    pub const L: Size = Size(4);
    pub const LARGEST: Size = Size(7);

    pub fn name(self) -> &'static str {
        NAMES[usize::from(self.0)]
    }

    pub fn parse(name: &str) -> Option<Size> {
        NAMES.iter().position(|n| n.eq_ignore_ascii_case(name.trim())).map(|n| Size(n as u8))
    }
}

impl Serialize for Size {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.name())
    }
}

impl Scale {
    // None when the team's type isn't one Linear documents.
    pub fn of(team: &Estimation) -> Option<Scale> {
        let kind = match team.issue_estimation_type.as_str() {
            "notUsed" => Kind::NotUsed,
            "exponential" => Kind::Exponential,
            "fibonacci" => Kind::Fibonacci,
            "linear" => Kind::Linear,
            "tShirt" => Kind::TShirt,
            _ => return None,
        };
        Some(Scale { kind, extended: team.issue_estimation_extended, zero: team.issue_estimation_allow_zero })
    }

    pub fn used(&self) -> bool {
        self.kind != Kind::NotUsed
    }

    // Every non-zero value of the scale's kind, extended or not: an issue estimated
    // before the team turned the extension off keeps its size.
    fn all(&self) -> &'static [u32] {
        match self.kind {
            Kind::NotUsed => &[],
            Kind::Exponential => &[1, 2, 4, 8, 16, 32, 64],
            Kind::Fibonacci | Kind::TShirt => &T_SHIRT,
            Kind::Linear => &[1, 2, 3, 4, 5, 6, 7],
        }
    }

    pub fn values(&self) -> Vec<u32> {
        let count = if self.extended { 7 } else { 5 };
        let zero = (self.used() && self.zero).then_some(0);
        zero.into_iter().chain(self.all().iter().take(count).copied()).collect()
    }

    // The size of an estimate of `points`: its position on the scale, or the position of
    // the largest value below it when it is off the scale (the team changed scales since).
    // None when the team doesn't use estimates, whatever the issue still carries.
    pub fn size(&self, points: f64) -> Option<Size> {
        self.used().then(|| Size(self.all().iter().filter(|&&p| f64::from(p) <= points).count() as u8))
    }

    // Err says why not.
    pub fn check(&self, points: u32) -> Result<(), String> {
        match self.describe() {
            None => Err("the issue's team doesn't use estimates".into()),
            Some(_) if self.values().contains(&points) => Ok(()),
            Some(values) => Err(format!("{points} isn't on the issue's team's estimate scale: {values}")),
        }
    }

    // The values as a brief gives them, e.g. "1 (XS), 2 (S), 3 (M), 5 (L) or 8 (XL)". None
    // when the team doesn't use estimates.
    pub fn describe(&self) -> Option<String> {
        if !self.used() {
            return None;
        }
        let values = self.values();
        let words: Vec<String> = match self.kind {
            Kind::TShirt => values.iter().map(|&p| format!("{p} ({})", self.size(f64::from(p)).map_or("", |s| s.name()))).collect(),
            _ => values.iter().map(u32::to_string).collect(),
        };
        let (last, rest) = words.split_last()?;
        let list = if rest.is_empty() { last.clone() } else { format!("{} or {last}", rest.join(", ")) };
        Some(if self.kind == Kind::TShirt { list } else { format!("{list} points") })
    }
}

#[cfg(test)]
mod tests;

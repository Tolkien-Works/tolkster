// Chosen once and stored with the worker's record, so a resume or a review round launches
// it as it started, whatever the issue or the ladder says by then, unless the claim had no
// size for it (executor::relaunch). Priority plays no part: it says how soon, not how hard.
// A rung names its harness (harness.rs), and an `agent:` label keeps to the rungs on one; a
// label for a harness no rung runs on is logged and ignored.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::config::{keys, whole_number};
use crate::error::Error;
use crate::estimates::Size;
use crate::harnesses::{DEFAULT, Harness};
use crate::trace::RungNote;

// Only ever from configuration, never from Linear's text: the harness's name, a model as its
// model flag takes it, and one of its efforts, as it spells it (harness::check). A record with
// no harness, as one written before the field was, is the default's.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Launch {
    #[serde(default = "default_harness")]
    pub harness: String,
    pub model: String,
    pub effort: String,
}

fn default_harness() -> String {
    DEFAULT.name.into()
}

impl Launch {
    pub fn new(harness: &Harness, model: &str, effort: &str) -> Launch {
        Launch { harness: harness.name.into(), model: model.into(), effort: effort.into() }
    }

    // The flags `tolkster worker` takes for it (setup.rs), the default harness's without its
    // own. Plain, once harness::check has it.
    pub fn flags(&self) -> String {
        let harness = if self.harness == DEFAULT.name { String::new() } else { format!("--harness {} ", self.harness) };
        format!("{harness}--model {} --effort {}", self.model, self.effort)
    }
}

// `up_to` is the largest size the rung takes; None: any.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Rung {
    pub name: String,
    #[serde(flatten)]
    pub launch: Launch,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub up_to: Option<Size>,
}

impl Rung {
    pub fn running_as(&self) -> String {
        format!("{}, {} effort", self.name, self.launch.effort)
    }

    fn is_model(&self, value: &str) -> bool {
        let first_word = self.name.split(' ').next().unwrap_or_default();
        [self.launch.model.as_str(), self.name.as_str(), first_word].iter().any(|m| m.eq_ignore_ascii_case(value))
    }
}

// `default`, counted from 1, is the rung an issue with no size, or a label off the ladder,
// takes.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Ladder {
    pub rungs: Vec<Rung>,
    pub default: usize,
}

const MAX_RUNGS: usize = 10;
pub(crate) const LADDER: &[&str] = &["rungs", "default"];
pub(crate) const RUNG: &[&str] = &["name", "harness", "model", "effort", "upTo"];

impl Default for Ladder {
    fn default() -> Ladder {
        let rung = |name: &str, model: &str, effort, up_to| Rung { name: name.into(), launch: Launch::new(DEFAULT, model, effort), up_to };
        Ladder {
            rungs: vec![
                rung("Sonnet 5.5", "claude-sonnet-5-5", "medium", Some(Size::XS)),
                rung("Sonnet 5.5", "claude-sonnet-5-5", "high", Some(Size::S)),
                rung("Opus 5.5", "claude-opus-5-5", "medium", Some(Size::M)),
                rung("Opus 5.5", "claude-opus-5-5", "high", None),
            ],
            default: 3,
        }
    }
}

impl Ladder {
    // Every value is checked here, so the command built from one is a plain line, and so is
    // the order: `upTo` rises from rung to rung and only the last may leave it out, so every
    // rung can be reached.
    pub fn from_config(value: Option<&Value>) -> Result<Ladder, Error> {
        let Some(value) = value else { return Ok(Ladder::default()) };
        let bad = |why: &str| Error::Usage(format!("\"ladder\" in config.json {why}"));
        keys(value, "ladder", LADDER, &[])?;
        let rungs = value.get("rungs").and_then(Value::as_array).filter(|r| (1..=MAX_RUNGS).contains(&r.len())).ok_or_else(|| bad(&format!("must have \"rungs\", a list of 1 to {MAX_RUNGS}")))?;
        let rungs = rungs.iter().enumerate().map(|(n, rung)| {
            let field = |name: &str| rung.get(name).and_then(Value::as_str);
            let at = |why: &str| bad(&format!("rung {}: {why}", n + 1));
            keys(rung, &format!("ladder.rungs[{n}]"), RUNG, &[])?;
            let name = field("name").filter(|s| plain_name(s)).ok_or_else(|| at("\"name\" must be letters, digits, spaces, dots and dashes, up to 40"))?;
            let harness = match field("harness") {
                None => DEFAULT,
                Some(name) => crate::harnesses::named(name).ok_or_else(|| at("\"harness\" must be one this build launches, or left out for claude"))?,
            };
            if !harness.unmet().is_empty() {
                return Err(at(&format!("\"harness\" {} can't run unattended safely: {}", harness.name, harness.unmet().join("; "))));
            }
            let model = field("model").filter(|s| plain_model(s)).ok_or_else(|| at(&format!("\"model\" must be a model's name or alias, as `{} {}` takes it", harness.program, harness.model_flag)))?;
            let effort = field("effort").and_then(|e| harness.effort(e)).ok_or_else(|| at(&format!("\"effort\" must be one of {}'s: {}", harness.name, harness.efforts.join(", "))))?;
            let up_to = match rung.get("upTo") {
                None | Some(Value::Null) => None,
                Some(v) => Some(v.as_str().and_then(Size::parse).ok_or_else(|| at("\"upTo\" must be a size, zero or XS, S, M, L, XL, XXL or XXXL"))?),
            };
            Ok(Rung { name: name.into(), launch: Launch::new(harness, model, effort), up_to })
        });
        let rungs = rungs.collect::<Result<Vec<_>, Error>>()?;
        let reachable = rungs.windows(2).all(|pair| match (pair[0].up_to, pair[1].up_to) {
            (Some(lower), Some(upper)) => lower < upper,
            (Some(lower), None) => lower < Size::LARGEST,
            (None, _) => false,
        });
        if !reachable {
            return Err(bad("must have \"upTo\" rising from rung to rung, as sizes, left out only on the last"));
        }
        let default = value.get("default").and_then(|n| whole_number(n, 1)).and_then(|n| usize::try_from(n).ok()).filter(|n| (1..=rungs.len()).contains(n)).ok_or_else(|| bad("must name its \"default\" rung, counted from 1"))?;
        Ok(Ladder { rungs, default })
    }

    // None takes the default.
    fn by_size(&self, size: Option<Size>) -> usize {
        let Some(size) = size else { return self.default - 1 };
        self.rungs.iter().position(|r| r.up_to.is_none_or(|up_to| size <= up_to)).unwrap_or(self.rungs.len() - 1)
    }

    // `agent:`, `model:` and `effort:` labels keep to the rungs that match them, and of those
    // the nearest to the size's, the stronger on a tie. A model or effort label that matches
    // no rung, or labels no rung matches together, give the default rung; an agent label that
    // names no rung's harness is ignored. No label's text is logged.
    pub fn choose(&self, size: Option<Size>, labels: &[String]) -> (&Rung, Vec<RungNote>) {
        let mut notes = Vec::new();
        let (mut agents, mut models, mut efforts) = (Vec::new(), Vec::new(), Vec::new());
        let mut off_ladder = false;
        for label in labels {
            let Some((key, value)) = label.split_once(':').map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim())) else { continue };
            match key.as_str() {
                "model" if self.rungs.iter().any(|r| r.is_model(value)) => models.push(value),
                "effort" if self.rungs.iter().any(|r| r.launch.effort.eq_ignore_ascii_case(value)) => efforts.push(value),
                "model" | "effort" => {
                    off_ladder = true;
                    notes.push(RungNote::Unmatched(if key == "model" { "model" } else { "effort" }));
                }
                "agent" if self.rungs.iter().any(|r| r.launch.harness.eq_ignore_ascii_case(value)) => agents.push(value),
                "agent" => notes.push(RungNote::Agent),
                _ => {}
            }
        }
        let default = &self.rungs[self.default - 1];
        if off_ladder {
            return (default, notes);
        }
        let wanted = self.by_size(size);
        let any = |values: &[&str], matches: &dyn Fn(&str) -> bool| values.is_empty() || values.iter().any(|v| matches(v));
        let matching = |r: &Rung| any(&agents, &|a| r.launch.harness.eq_ignore_ascii_case(a)) && any(&models, &|m| r.is_model(m)) && any(&efforts, &|e| r.launch.effort.eq_ignore_ascii_case(e));
        let nearest = self.rungs.iter().enumerate().filter(|(_, r)| matching(r)).min_by_key(|(n, _)| (n.abs_diff(wanted), usize::MAX - n)).map(|(_, r)| r);
        match nearest {
            Some(rung) => (rung, notes),
            None => {
                notes.push(RungNote::Apart);
                (default, notes)
            }
        }
    }
}

// A model as a harness's model flag takes it, and as the Durable Object stores it (MODEL in
// src/worker/state.ts): a letter or digit, then letters, digits, dots, dashes and
// underscores, 64 in all at most.
pub fn plain_model(s: &str) -> bool {
    s.len() <= 64 && s.starts_with(|c: char| c.is_ascii_alphanumeric()) && s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
}

fn plain_name(s: &str) -> bool {
    s.len() <= 40 && s.starts_with(|c: char| c.is_ascii_alphanumeric()) && s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, ' ' | '.' | '-'))
}

#[cfg(test)]
mod tests;

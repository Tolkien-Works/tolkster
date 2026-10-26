// The harnesses this build launches, from code alone (docs/harness.md): what each is, which
// a rung in config.json names and is checked against (config/ladder.rs), and `host add`'s steps for
// it. Launching one is harness.rs's.

// A harness a worker can run on, from code alone: its name, which a rung, an `agent:` label
// and a worker's record give it (config/ladder.rs), the program it is launched as, the flag that
// resumes the worktree's last session, its model and effort flags, and the effort levels it
// takes, weakest first. `settings`: it starts with the `tolkster hook` settings and the MCP
// file (`harness::args`) and keeps a folder's trust in `settings_file` (`harness::trust`), so
// its events reach the hooks. `token_env`: the variable the owner's token goes in. `host`: its `host add` steps,
// whose unattended check is how it blocks risky actions with no person to answer. `unattended`:
// the flags a harness without `settings` always starts with, that skip its trust dialog and
// start no MCP servers. `unmet` lists the unattended-safety requirements it lacks.
#[derive(Debug, PartialEq)]
pub struct Harness {
    pub name: &'static str,
    pub program: &'static str,
    pub resume: &'static str,
    pub model_flag: &'static str,
    pub effort_flag: &'static str,
    pub efforts: &'static [&'static str],
    pub settings: bool,
    pub token_env: Option<&'static str>,
    pub host: Option<&'static HostSteps>,
    pub unattended: &'static [&'static str],
}

// Claude Code's program, which a conversation's turn runs too (`harness::turn`).
pub const PROGRAM: &str = "claude";

pub const CLAUDE: Harness = Harness {
    name: "claude",
    program: PROGRAM,
    resume: "--continue",
    model_flag: "--model",
    effort_flag: "--effort",
    efforts: &["low", "medium", "high", "xhigh", "max"],
    settings: true,
    token_env: Some(TOKEN_ENV),
    host: Some(&HOST),
    unattended: &[],
};

// The harness of a rung that names none, and of a launch with no record.
pub const DEFAULT: &Harness = &CLAUDE;

// The harnesses this build launches, which src/worker/state.ts's HARNESSES twins.
pub const KNOWN: [&Harness; 1] = [&CLAUDE];

// The harness this build launches by that name, if any: in tests, stubs besides them.
pub fn named(name: &str) -> Option<&'static Harness> {
    #[cfg(not(test))]
    let mut known = KNOWN.into_iter();
    #[cfg(test)]
    let mut known = KNOWN.into_iter().chain([&tests::STUB, &tests::BARE]);
    known.find(|harness| harness.name.eq_ignore_ascii_case(name))
}

impl Harness {
    // The level as the harness spells it, if it takes it.
    pub fn effort(&self, effort: &str) -> Option<&'static str> {
        self.efforts.iter().copied().find(|e| e.eq_ignore_ascii_case(effort))
    }

    // The unattended-safety requirements (docs/harness.md) this harness doesn't meet, as the
    // reasons `doctor` reports; a launch of one that lacks any is refused (`harness::check`).
    pub fn unmet(&self) -> Vec<&'static str> {
        let mut unmet = Vec::new();
        if self.host.is_none() {
            unmet.push("no check on a host that it blocks risky actions with no person to answer");
        }
        if !self.settings && self.unattended.is_empty() {
            unmet.push("no settings of Claude's kind and no flags of its own that skip the trust dialog and start no MCP servers");
        }
        unmet
    }
}

// The variable Claude Code reads a `claude setup-token` token from.
pub const TOKEN_ENV: &str = "CLAUDE_CODE_OAUTH_TOKEN";

// A command `host add` runs on a worker host, for a POSIX shell and for PowerShell, each
// ending on the status it means as onboard.rs's own do.
#[derive(Debug, PartialEq)]
pub struct Shells {
    pub posix: &'static str,
    pub powershell: &'static str,
}

impl Shells {
    pub fn on(&self, windows: bool) -> &'static str {
        if windows { self.powershell } else { self.posix }
    }
}

// `host add`'s steps for the harness: whether it is installed, its installer, and whether it
// runs unattended, which a worker needs: a prompt it waits at has no one to answer it. The
// report's step names and reasons are `host add`'s JSON.
#[derive(Debug, PartialEq)]
pub struct HostSteps {
    pub installed: Shells,
    pub install: Shells,
    pub unattended: Shells,
    pub step: &'static str,
    pub installed_by: &'static str,
    pub install_failed: &'static str,
    pub unattended_step: &'static str,
    pub unattended_ok: &'static str,
    pub unattended_advice: &'static str,
}

pub const HOST: HostSteps = HostSteps {
    installed: Shells { posix: "command -v claude >/dev/null 2>&1", powershell: "$LASTEXITCODE = [int](-not (Get-Command claude -ErrorAction SilentlyContinue))" },
    install: Shells { posix: "curl -fsSL https://claude.ai/install.sh | bash", powershell: "try { irm https://claude.ai/install.ps1 | iex; $LASTEXITCODE = 0 } catch { $LASTEXITCODE = 1 }" },
    unattended: Shells {
        posix: "grep -Eq '\"defaultMode\"[[:space:]]*:[[:space:]]*\"auto\"' \"$HOME/.claude/settings.json\"",
        powershell: "$LASTEXITCODE = [int](-not ((Get-Content \"$HOME/.claude/settings.json\" -Raw -ErrorAction SilentlyContinue | ConvertFrom-Json).permissions.defaultMode -eq 'auto'))",
    },
    step: PROGRAM,
    installed_by: "installed with Claude Code's native installer",
    install_failed: "Claude Code's installer failed, or left claude off PATH: add ~/.local/bin to PATH on the host",
    unattended_step: "auto mode",
    unattended_ok: "Claude's default mode is auto",
    unattended_advice: "set Claude's default mode to auto on the host: permissions.defaultMode in ~/.claude/settings.json",
};

#[cfg(test)]
pub mod tests;

use super::*;

// A second harness, which only tests launch: its own program and flags, and none of
// Claude's files or its token, with flags of its own for what Claude's settings do.
pub const STUB: Harness = Harness { name: "stub", program: "stub-agent", resume: "--resume-last", model_flag: "-m", effort_flag: "--depth", efforts: &["shallow", "deep"], settings: false, token_env: None, host: Some(&HOST), unattended: &["--no-trust-dialog", "--no-mcp"] };

// A harness that meets neither unattended-safety requirement.
pub const BARE: Harness = Harness { name: "bare", unattended: &[], host: None, ..STUB };

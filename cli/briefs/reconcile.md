## Reconcile first

<identifier> was written, or its description last changed, at <baseline> (UTC).
Since then <commits> landed on the default branch: `<range>`. The issue may describe
code that has since moved, so check it against the code as it is now before you do
anything else, "Before any work" included.

1. Read what landed with `git log --stat <range>`, and `git log -p <range> -- <path>`
   where it touches what the issue names. Compare what the issue asks for with what the
   code does now.
2. **Nothing moved, or only a rename, a move or part already done**: carry on with the
   brief, adapting to the code as it is, and say in your task thought what moved and how.
3. **A later change took another approach, the issue contradicts the code, or all of it
   is done**: ask <OWNER NAME> before changing anything, the estimate included,
   and set it as agreed after the go.
   - If this brief has a "Plan first" section, follow it, and cover this in its
     elicitation too: one elicitation, not two.
   - Otherwise, your branch isn't pushed yet, and stays unpushed until your reply's
     go: <HOLD> Then post an
     `elicitation` with `--body-file -` and `--option go`: what moved, with its PR numbers, how you now read the task,
     and what you propose, or closing it if it's all done. End it: "Reply 'go' or tell me what to change." Then print
     `<identifier> PLAN POSTED` and close your terminal in the same command, as in
     Finishing's last step.
     The reply resumes you where you stopped.
   - <APPROVED>

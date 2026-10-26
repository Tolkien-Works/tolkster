# <identifier>: rebase round

PR #<n> is behind its base or conflicts with it, and nothing else is new: this round
rebases it and does nothing more. Your task brief, `<identifier>.md` beside this file,
still holds; if you've read it this session, don't read it again, nor your own transcript.

1. **Open in one turn**, as parallel calls, before you change anything: `tolkster
   activity <sessionId> thought --state rebasing`, `git ls-remote origin
   refs/heads/<branchName>`, `git fetch origin main <branchName>` and `tolkster pr
   checks --pr <n>`. Exit 5 or 6 ends the round as "Pushing in a round" says; exit 7 is
   expected.
2. Run the ancestor check ("Pushing in a round"), then rebase as "Rebasing" says.
3. <ROUND PUSH>
4. Run `tolkster in-review <sessionId>`, which moves <identifier> back to review, then
   follow Finishing from step 3. Leave auto-merge as it is.

<ROUND GIT>

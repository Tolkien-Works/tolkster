Before you push what you committed or rebased, give the round's changes the light pass
   from Finishing step 1: `git diff origin/<branchName>...HEAD` shows the round's
   commits until you push; after a rebase, read your round's commits (`git show`
   each) and the files whose conflicts you resolved instead. Fix what you find and
   commit. Keep its line for the round's `response`, e.g. "Round review: 1 finding,
   fixed." Then push with the lease ("Pushing in a round"). Don't run the
   pre-push review's deep pass here: it's only for the PR's opening.

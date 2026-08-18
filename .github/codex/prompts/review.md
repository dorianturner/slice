Review the current change diff against the merge base with `origin/main` and
against AGENTS.md, ARCHITECTURE.md, docs/security.md, and docs/testing.md.
For a pull request, inspect `git diff $(git merge-base origin/main HEAD) HEAD`;
for a merge-group run, inspect the checked-out merge-group commit against
`origin/main`.

Check dependency layering, unsafe-code boundaries, profile-format compatibility,
tests, documentation, workflow permissions, and security. Do not edit files.

Your output must begin with exactly one of these lines:

RESULT: PASS
RESULT: FAIL

Use RESULT: FAIL for any unresolved must-fix issue. After the result line,
provide concise findings and required remediation. RESULT: PASS is allowed only
when no must-fix issue remains.

# Keeping personal files out of Git

Keep music collections outside the repository. Local reports belong in `outputs/` or `test-reports/`; use `local-data/` or `staging/` for disposable development copies. These directories and `.env` files are ignored. A force-add can bypass ignore rules, so every commit also needs a staged-file check.

## Before committing

Run from the repository root:

```sh
python3 scripts/check-private-files.py
```

The command reads the Git index, including data already staged even if the working copy has since been cleaned. It rejects local report directories, music files and DJ databases. It also checks for common credential formats and personal home-directory paths. Failures name the file and rule without printing matching contents.

To enable the included pre-commit hook in a checkout that has no existing hook configuration:

```sh
 git config --local core.hooksPath scripts/hooks
```

If a hook is already installed, add the scanner invocation to that hook instead of replacing it. The hook requires Python 3. A linked worktree shares repository configuration, so enable this hook only after the script exists on every branch being used, or use the explicit scanner command while branches are under review.

CI runs the scanner with `--all`, which checks every tracked file. This check adds a job alongside the six engine and desktop jobs. A CI failure happens after upload, so run the staged check locally before the first commit or push.

## Regression fixtures

Raw diagnostics stay local because they can contain serial numbers, volume names and machine paths. Reduce a report to the nodes needed to reproduce the bug, replace identifiers with synthetic values, and remove unrelated devices. Review the reduced data before committing it.

Diagnostic JSON requires an exact SHA-256 entry in `scripts/privacy-fixtures.json`. That file records reviewed fixture bytes; changing a fixture invalidates its approval. Updating a hash requires another content review. The existing Kingston hub fixture is approved after replacing serial numbers and volume labels and removing UUIDs and internal disks.

## Review boundaries

The scanner is a guard against common accidental commits. It cannot identify every personal document or secret. Review images and arbitrary text manually, including generated mock data. Synthetic test identities such as `/Users/example/` and `/Users/dj/` are allowed; never substitute a real home directory.

Git commit metadata is separate from file contents. Configure this repository to use the account's GitHub noreply email before committing. Changing that setting affects future commits. Earlier commits retain their original author and committer addresses until their history is rewritten.

Do not include raw report content in PR descriptions, CI logs or screenshots. Keep OAuth credentials in the OS credential store when cloud integration is implemented.

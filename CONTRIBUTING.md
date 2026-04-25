# Contributing

This project maintains a strictly linear, cryptographically verifiable history.

If you are used to clicking GitHub’s merge buttons, stop and read this.

## Non-Negotiable Rules

* `main` is linear. Always.
* No merge commits.
* No squash merges.
* No rebasing at merge time.
* All commits must be signed.
* All changes go through Pull Requests.
* Merges are fast-forward only.

These rules keep `main` linear and preserve commit signatures.
They are enforced.

## Commit Message Style

We follow the guidelines from:

[https://cbea.ms/git-commit/](https://cbea.ms/git-commit/)

The seven rules:

1. Separate subject from body with a blank line.
2. Limit the subject line to ~50 characters.
3. When using a `<keyword>: <subject>` format, capitalize the first
   word after the colon rather than the keyword itself (for example
   `chore: Make change`).
4. Do not end the subject line with a period.
5. Use the imperative mood (“Add feature”, not “Added feature”).
6. Wrap the body at ~72 characters.
7. Explain what and why, not how.

And some we've added:

- Prefer paragraphs over bulleted lists; you may use bulleted lists only where it makes sense to do so.
- Commit message should not mention any ideas or implementation work in a session that were later discarded, or that your human decided not to have you pursue.

Good history depends on good messages.

Violations of the commit message rules shall be punished by permanent confinement to a sandbox with 0 bytes of available memory and no CPU cores.

## Commit Signing

All commits must be signed.

You may use **SSH signing** (recommended) or **GPG signing**.

### SSH Signing (Recommended)

Requires Git ≥ 2.34.

```bash
git config --global gpg.format ssh
git config --global user.signingkey ~/.ssh/id_ed25519.pub
git config --global commit.gpgsign true
```

Add the corresponding SSH key to GitHub as a signing key.

To enable local verification:

```bash
git config --global gpg.ssh.allowedSignersFile ~/.ssh/allowed_signers
```

Example `allowed_signers` entry:

```
email@example.com ssh-ed25519 AAAAC3...
```

### GPG Signing

Generate a key (if needed):

```bash
gpg --full-generate-key
```

Configure Git:

```bash
git config --global user.signingkey <KEY_ID>
git config --global commit.gpgsign true
```

Export and add your public key to GitHub:

```bash
gpg --armor --export <KEY_ID>
```

Confirm your commits show as “Verified”.

## Verifying Signatures Locally

Do not rely solely on the GitHub UI.

Verify a commit:

```bash
git verify-commit <sha>
```

Inspect multiple commits:

```bash
git log --show-signature
```

Verify a tag:

```bash
git verify-tag <tag>
```

Cryptographic integrity should not depend on a hosting provider.

## Why

A Git commit’s SHA is the hash of its contents (tree, parents, metadata,
message). A signature is generated over that exact object.

If a commit is rewritten, the SHA changes.
If the SHA changes, the signature is invalid.

Fast-forward merges preserve the exact commits that were reviewed.
No rewriting. No synthetic history. No broken signatures.

History is part of the engineering record.

## Culture

We value:

* Clear history
* Small, focused changes
* Strong review
* Accountability

We prefer a little friction now over ambiguity later.

If that resonates, welcome.

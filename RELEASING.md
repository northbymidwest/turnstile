# Releasing

Run the `release` workflow from the Actions tab with the version, no leading `v`.

Two switches, both on by default. `dry_run` builds, signs and packages without
notarizing or publishing, so the build can be proved before spending the round trip to
Apple. `draft` creates the release as a draft: GitHub creates no tag for a draft, so the
disk image can be downloaded and opened on a real machine before anything is permanent,
and publishing the draft is what creates the tag.

So the usual path is one run with `dry_run` on, one with it off and `draft` on, then a
look at the artifact, then publish the draft.

A `preflight` job runs the checks first, with no write scope and no access to the
secrets, and the `release` environment's approval gate sits between it and the job that
signs. Approving means approving a release that has already passed.

## Before the first release

Five secrets on the `release` environment. Two scripts set them, and both check what
they are given before uploading anything:

```
./scripts/set-signing-secrets.sh path/to/Certificates.p12
./scripts/set-notary-secrets.sh  path/to/AuthKey_XXXXXXXXXX.p8
```

The certificate is a `Developer ID Application` export from Keychain Access. The key is
an App Store Connect API key with the Developer role, from Users and Access,
Integrations; its `.p8` downloads exactly once.

Each script's header says what it checks and why.

## Everything else

The steps are composite actions in
[northbymidwest/gh-actions](https://github.com/northbymidwest/gh-actions), and each one
documents itself: `macos-signing-keychain`, `macos-dmg`, `macos-notarize`,
`release-preflight`, `release-review-summary`.

`scripts/bundle.sh` builds and signs, `scripts/notarize.sh` submits and staples, and
both run standalone. `scripts/make-dmg-layout.sh` regenerates the disk image's icon
layout, which has to happen on a Mac with a Finder and is why the result is committed.

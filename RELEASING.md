# Releasing

A release is a universal, Developer ID signed, notarized `Turnstile.app`, published
inside a `.dmg` on the GitHub releases page. Nothing goes to crates.io: both crates are
`publish = false`, because the deliverable is the application.

The shared steps are composite actions in
[northbymidwest/gh-actions](https://github.com/northbymidwest/gh-actions), pinned by
commit like any other action: `macos-signing-keychain`, `macos-dmg` and
`macos-notarize`.

Only the disk image is submitted. The notary service scans inside it, so the ticket it
returns covers the application too: a loose copy of the app can be stapled afterwards
without ever having been submitted on its own, which was checked rather than assumed.

The application inside the shipped image is left unstapled. That costs an offline first
launch after somebody drags it to Applications, and nothing else; Gatekeeper reports it
as `Notarized Developer ID` on any machine that can reach Apple. Stapling it as well
would mean rebuilding the image afterwards, and the rebuilt image's own ticket would no
longer match its bytes, so it would need a second submission of its own.

Run the `release` workflow from the Actions tab with the version, no leading `v`. Two
switches, both on by default:

`dry_run` builds, signs and packages without notarizing or publishing. Use it to prove
the build before spending the notarization round trips.

`draft` creates the release as a draft rather than publishing it. This is the one to
leave on: GitHub does not create a tag for a draft at all, so the run leaves nothing
permanent behind, and the disk image can be downloaded from the draft and opened on a
real machine before anybody else sees it. Publishing the draft in the web interface is
what creates the tag.

So the usual path is one run with `dry_run` on to check the build, then one with it off
and `draft` on, then a look at the artifact, then publish the draft.

## The two jobs

`preflight` runs first, on Ubuntu, with no write scope and no access to the signing or
notary secrets. Every check in it comes from `release-preflight`, the same action the
crate releases use: the version is well formed and matches `Cargo.toml`, the tag does
not exist and neither does a release for it, `CHANGELOG.md` has a non-empty section for
the version, and the newest CI run for this commit is green, waiting for it to finish if
it is still running. Then `release-review-summary` writes the table an approver reads.

Two of its inputs are set the way they are for a reason. `crates` is empty and
`manifests` names `Cargo.toml` instead: `crates` means crates being published, and
asserts `publish = false` has been removed, which is right for crates.io and exactly
wrong here, where both crates stay unpublishable because the deliverable is the
application. `manifests` is the version half of that check without the assertion.
`refuse-existing-release` is on because releases here are drafts by default, and a draft
holds its tag name without creating the tag.

`release` needs `preflight` and is gated by the `release` environment. The gate sits
between the two on purpose: approving means approving a release that has already passed
its checks, rather than approving before anything has been looked at. It is also the
only job the secrets are reachable from.

`CHANGELOG.md` needs a `## <version>` section before a release will pass preflight. Its
contents are not used for the release notes, which are generated with the checksum, but
an empty or missing section stops the release.

The tag is created by a release that succeeded. It is never the trigger: a tag pointing
at a build that failed to notarize is worse than no tag.

Re-running a version is refused, on both the tag and an existing release. The two can
disagree, because a draft holds its tag name without creating the tag; delete the draft
first if you mean to rebuild that version.

## Before the first release

Create the repository and push, then set up the `release` environment and its secrets.
The workflow will not run without them, and the environment is where the approval gate
lives.

### Signing certificate

A `Developer ID Application` certificate, exported from Keychain Access as a `.p12`
with a password.

| Secret | What it holds |
|---|---|
| `MACOS_CERT_P12` | the `.p12`, base64 encoded: `base64 -i cert.p12 \| pbcopy` |
| `MACOS_CERT_PASSWORD` | the password set when exporting it |
| `MACOS_SIGN_IDENTITY` | the identity's full name, e.g. `Developer ID Application: Your Name (TEAMID1234)` |

`security find-identity -v -p codesigning` prints the exact identity string.

### Notarization

An App Store Connect API key, not an Apple ID and app-specific password. A key belongs
to the team rather than to a person, can be revoked on its own, and does not break when
somebody enables two-factor or leaves.

Create one under Users and Access, Integrations, in App Store Connect, with the
Developer role. The `.p8` downloads once and cannot be downloaded again.

| Secret | What it holds |
|---|---|
| `NOTARY_KEY_P8` | the contents of the `.p8` file, not a path |
| `NOTARY_KEY_ID` | the key ID, the 10 characters in the filename |
| `NOTARY_ISSUER_ID` | the issuer UUID, shown above the key list |

`scripts/notarize.sh` takes these from the environment when they are set, and otherwise
falls back to a local keychain profile, so the same script runs in CI and on a laptop.

## Releasing by hand

The workflow is the supported path, but the scripts it calls work standalone:

```
./scripts/bundle.sh     # universal, signed if a Developer ID identity is in the keychain
./scripts/notarize.sh   # submits, waits, staples, then verifies as Gatekeeper will
```

## The disk image window

`resources/dmg-DS_Store` is what positions the icons and sizes the window Finder opens:
Turnstile on the left, the Applications drop target on the right, in the order the drag
happens.

Only Finder writes such a file, and only through AppleScript, so it cannot be produced
on a CI runner. It is generated once on a real Mac and committed:

```
./scripts/bundle.sh
./scripts/make-dmg-layout.sh
```

It opens a Finder window briefly while it works. Regenerate it only when the layout
should change, or when something in the image is renamed, since the positions are
recorded against item names.

`bundle.sh` needs full Xcode rather than the Command Line Tools, because `actool`
compiles the icon and ships only with Xcode. Without it the build still succeeds and
says loudly that it has no icon.

## What the release workflow checks

Each of these has been wrong at some point, so each is asserted rather than assumed:

- the version given on the dispatch matches `Cargo.toml`
- the tag does not already exist
- the whole test suite passes
- the built binary is universal, `x86_64 arm64`
- the signature is a real Developer ID one with a team identifier, not ad-hoc
- Gatekeeper accepts it, and the notarization ticket is stapled so it verifies offline

The signing certificate is imported into a throwaway keychain that is deleted in an
`always()` step, so a failure part way through does not leave a private key on a runner.

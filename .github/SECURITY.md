# Security policy

## Supported versions

Turnstile is 0.x. Fixes go into a new release cut from `main`; older versions
are not patched. Report against the latest release or against `main`.

## What this application does

Turnstile fetches release metadata from the GitHub API over HTTPS, downloads
build archives from the URLs that metadata gives, unpacks them with
`/usr/bin/ditto` or `tar`, and launches the resulting executable. It writes
into `~/Library/Application Support/OpenRCT2` and `.../OpenLoco`, which are the
directories holding your saved games, scenarios and configuration.

`turnstile-core` is `#![forbid(unsafe_code)]`. The application crate uses
`unsafe` only where AppKit requires it through `objc2`.

Releases are universal binaries signed with a Developer ID certificate and
notarized by Apple.

## In scope

- Anything that lets a response from the network decide what gets written, or
  where: a path escaping the install directory, an archive member unpacking
  outside it, a download replacing something it should not.
- Loss of a game build, or of saved games, scenarios or configuration
  alongside it.
- Launching something other than the build that was installed.
- The release path: the workflow, its signing and notarization, or a published
  disk image whose contents do not match the commit its tag names.

## Not in scope

These are documented behaviour. A report about one is a bug report or a
question, and is welcome as a normal issue:

- Turnstile replacing an existing installation when installing a build. That
  is what it is for; in the default mode there is one install per game.
- Turning off "keep multiple versions installed" leaving the other builds on
  disk rather than deleting them. Deliberate.
- Trusting the OpenRCT2 and OpenLoco GitHub organizations to publish their own
  releases. Turnstile verifies the transport, not the contents.

## Reporting

Use GitHub's private vulnerability reporting: the **Security** tab of this
repository, then **Report a vulnerability**. That keeps the report private
until there is a release to point at.

Include the Turnstile version, the macOS version, whether "keep multiple
versions installed" was on, and what the install directory looked like before
and after.

This is a personal project maintained by one person. Expect a reply in days
rather than hours, and nothing more binding than that.

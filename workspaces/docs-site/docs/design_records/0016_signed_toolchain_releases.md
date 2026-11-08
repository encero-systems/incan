---
id: DD-0016
title: Signed toolchain releases
status: Accepted
type: design-decision
date: 2026-11-07
review_target: v0.6
sources:
  - Maintainer decision of 2026-11-07 that toolchain releases are signed and every install channel verifies them
  - DD-0010
  - RFC 125
  - https://docs.sigstore.dev/
---

<!-- markdownlint-configure-file {"MD025": {"front_matter_title": ""}} -->

# DD-0016: Signed toolchain releases

## Context

DD-0010 makes a versioned release archive the thing that CI and development install, and it says plainly that it does not define artifact signing. Today an installer checks the archive's SHA-256 against the release manifest that sits beside the archive. A checksum beside the asset proves the download was not corrupted; it proves nothing about who produced the asset, because whoever can replace the archive can replace the manifest. Every install channel the project offers downloads a release this way: the install script, the npm shim, the pip package, the Homebrew formula and the editor extension. The registry (RFC 125) already attests its own builds through Sigstore, so the project has one provenance mechanism in use and a second class of artifact without it.

## Decision

1. **Every release is signed, keyless.** The release workflow signs each release asset and the release manifest through Sigstore from the workflow's own identity (the repository, the workflow file and the tag it ran for, as the OIDC token states them). No signing key exists to lose or rotate. This is the mechanism the registry's build attestations use.
2. **The signer identity is a release fact.** A release names the identity that signed it, and an installed toolchain carries that identity. The identity changes only through a signed release that states the new one.
3. **Upgrades verify before installing.** An installed toolchain verifies the signature bundle of every later release against the signer identity it carries, and the Sigstore trust root it carries, before it unpacks or installs anything. A release that fails verification is refused by name, never installed with a warning. This is the case that matters most: a release compromised after users have installed is stopped at every one of them.
4. **Every channel verifies and none falls back.** The npm shim, the pip package, the Homebrew formula and the editor extension each carry the expected signer identity and refuse a release whose bundle does not verify against it. No channel downgrades to checksum-only when verification is unavailable; an environment that cannot verify gets a refusal that says so.
5. **One verifier.** Verification is implemented once, in the toolchain (`incan toolchain install` and the upgrade path), and every channel invokes that verifier rather than carrying its own. The verifier checks the Sigstore bundle: the certificate chain to the Fulcio root, the transparency-log inclusion proof, the signer identity and issuer, and the digest of the asset.
6. **The install script is served from the project's own domain**, apart from the release assets on GitHub, and it is generated per release so that it carries the digest of that release's manifest. Swapping the script and swapping the assets are then two compromises on two origins, not one. The script's own guarantee is stated honestly: a shell script cannot verify a Sigstore bundle by itself, so a first install through `curl | sh` gets a separate-origin pinned digest, and signature verification of the downloaded release happens as soon as the downloaded toolchain exists and before it installs anything further. A first install that wants independent signature verification uses a channel whose own verifier is trusted already (an installed `incan`, `cosign`, or the GitHub CLI's attestation verifier), and the install page says how.
7. **Trust anchors travel in the signed archive.** The Sigstore trust root the verifier uses ships inside the toolchain archive and is updated only through signed releases. Once RFC 125's registry root key exists, the registry's root key and the registry pin travel the same way, so a verified toolchain is also the root of trust for the registry it talks to.
8. **Trusted publishing, no stored tokens.** The npm and PyPI packages are published through trusted publishing from the release workflow, with provenance, so the registries' own attestations name the same workflow identity and no long-lived publishing token exists anywhere.
9. **The install page states it.** The install documentation says that releases are signed, names the signer identity, and shows how to verify a release by hand with a standard verifier.

## Consequences

The release workflow gains a signing step and an identity it must keep stable; changing the workflow file's path or the repository renames the identity and requires a signed release that announces the new one first. The toolchain gains a verifier and a trust root, and installation gains a refusal path that names the release and the reason. Each channel gains the expected identity as data and loses its checksum-only code path. The install script moves to a project domain and becomes a release output. The first install's guarantee is weaker than an upgrade's and the documentation says which is which.

## Non-goals

This decision does not sign Loaves or registry assets (RFC 125 does), does not sign source commits, does not define a key-based alternative, and does not decide how the project holds its package names on npm, PyPI and Homebrew or their lookalikes, which is a maintainer action outside the repository.

## Revisit condition

Revisit when Sigstore's trust-root distribution changes, when a channel appears that cannot invoke the toolchain's verifier, or when the first install needs an independent verifier of its own.

## Provenance

This record derives from the maintainer's decision of 2026-11-07 and fills the gap DD-0010 left open; it supersedes nothing. RFC 125 is the authority for the registry's attestations and root key.

## References

- DD-0010: Released toolchains for CI and development
- RFC 125: the Loaf registry and baked asset distribution
- Sigstore keyless signing and bundle verification

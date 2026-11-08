---
id: DD-0016
title: Signed toolchain releases
status: Accepted
type: design-decision
date: 2026-11-07
review_target: v0.6
sources:
  - Maintainer decision of 2026-11-07 that toolchain releases are signed and every install channel verifies them
  - Maintainer clarification of 2026-11-08 on initial installation trust and verification before execution
  - DD-0010
  - RFC 125
  - https://docs.sigstore.dev/
---

<!-- markdownlint-configure-file {"MD025": {"front_matter_title": ""}} -->

# DD-0016: Signed toolchain releases

## Context

DD-0010 makes a versioned release archive the thing that CI and development install, and it says plainly that it does not define artifact signing. Today an installer checks the archive's SHA-256 against the release manifest that sits beside the archive. A checksum beside the asset proves the download was not corrupted; it proves nothing about who produced the asset, because whoever can replace the archive can replace the manifest. Every install channel the project offers downloads a release this way: the install script, the npm shim, the pip package, the Homebrew formula and the editor extension. The registry (RFC 125) already attests its own builds through Sigstore, so the project has one provenance mechanism in use and a second class of artifact without it.

## Decision

1. **Every release is signed, keyless.** The release workflow signs each release asset and the release manifest through Sigstore from the workflow's own identity (the repository, the workflow file and the tag it ran for, as the OIDC token states them). No long-lived private signing key needs to be stored by the project; keyless signing still uses cryptographic keys. This is the mechanism the registry's build attestations use.
2. **The expected signer comes from established trust.** An installed toolchain carries the authorized signer policy; a first-install channel supplies it through that channel's established trust mechanism. The downloaded release cannot authorize its own signer merely by naming one. Verification binds the repository, workflow and issuer to that policy and the signed tag to the requested release. A change of authorized signer requires a transition authenticated under the previously trusted policy.
3. **Upgrades verify before installing.** The already-installed toolchain verifies the signature bundles for a later release's manifest and assets against its trusted signer policy and Sigstore trust root before unpacking, installing or executing the new toolchain. A release that fails verification is refused by name, never installed with a warning. This detects replacement of published files by someone who cannot produce an authorized signature. It does not establish that code signed by a compromised authorized workflow is safe, or protect a machine whose installed verifier is already compromised.
4. **Every channel states its initial trust and none silently falls back.** The npm shim, pip package, Homebrew formula and editor extension must identify how the user obtains a trusted installer or verifier and its expected signer policy. Initial trust comes from an already-trusted operating system, package manager or installed verification tool, not from the release being checked. OS-signed installers may use the operating system's existing publisher-verification mechanism; package-manager installations rely on that channel's distribution and verification mechanism. A channel that downloads a separate toolchain archive must verify its Sigstore bundle before executing anything from that archive. If its required verification is unavailable, it refuses rather than silently falling back to a checksum. The explicitly weaker HTTPS script path is described separately below.
5. **One shared verification implementation, not self-verification.** Incan's bundle-verification implementation serves both installed toolchains (`incan toolchain install` and upgrades) and bootstrap installers where needed. A bootstrap verifier must itself arrive through the channel's established trust mechanism, independently of the archive it checks. A downloaded toolchain must never be executed to establish its own authenticity. An already-trusted standard verifier may instead verify the bundle under the same signer policy; channels need not invent their own cryptography. Verification checks the certificate chain to the Fulcio root, the transparency-log inclusion proof, the signer identity and issuer, and the asset digest.
6. **The convenient install script has an explicit HTTPS trust boundary.** The script is served from the project's own domain, apart from the release assets on GitHub, and generated per release with the digest of that release's manifest. This separates the locations serving the script and assets, but does not prove their administration or credentials are independent. Without an already-trusted verifier, a first install through `curl | sh` trusts the website, HTTPS delivery and script; its pinned digest detects an asset mismatch but is not independent signature verification. Downloading a verifier from that same source does not strengthen this initial trust. Subsequent upgrades use the installed toolchain's signature verification. Users requiring independent verification before the first execution use an already-trusted verifier such as `cosign`, or an installation channel with an established verification mechanism. A failed signature check never triggers a fallback to the weaker path.
7. **Trust anchors travel in the signed archive.** The Sigstore trust root ships inside the archive for subsequent verification and is updated through releases authenticated by the existing trust anchors. The new archive's own root cannot authenticate its first installation; that trust comes from the installation channel. Once RFC 125's registry root key exists, the registry's root key and pin travel the same way, so a verified toolchain supplies trust anchors for the registry it talks to.
8. **Trusted publishing, no stored tokens.** The npm and PyPI packages are published through trusted publishing from the release workflow, with provenance, so the registries' own attestations identify the publishing workflow and no long-lived publishing token is required. Publication provenance does not replace verification of a separately downloaded toolchain archive.
9. **The install page states the guarantees.** The documentation names the expected signer policy, explains the initial trust for each channel, distinguishes the HTTPS bootstrap from verified upgrades, and shows independent verification with a standard verifier. It does not imply that signatures prove program safety or remove the need for an initial trusted source.

For example, the installation flows differ in what is trusted before any downloaded toolchain code runs:

| Path | Already trusted | What happens before executing the downloaded toolchain |
| --- | --- | --- |
| Upgrade | Installed Incan and its signer policy and trust roots | Existing Incan verifies the release manifest and archive signatures. |
| Package manager or OS installer | The channel's existing verification mechanism and installer | The channel authenticates its installer; any separately downloaded toolchain archive is signature-verified by a trusted verifier. |
| Independent manual verification | An installed standard verifier and an independently obtained expected signer policy | The verifier checks the release bundle before the user installs or runs the toolchain. |
| Convenient `curl \| sh` bootstrap without a trusted verifier | Project website, HTTPS infrastructure and delivered script | The script checks its pinned manifest and archive digests; independent signature verification is not claimed. |

## Consequences

The release workflow gains a signing step and a signer policy it must keep stable; changing the workflow file's path or repository requires an authenticated policy transition. The toolchain gains shared verification logic and trust anchors, and installation gains a refusal path that names the release and reason. Each channel documents and implements its own initial trust path while sharing Incan's verification implementation where applicable. No channel can establish independent trust by running code from the archive it is checking. The install script moves to a project domain and becomes a release output; its HTTPS-based first-install guarantee remains explicitly weaker than verification by an already-trusted toolchain or installer.

## Non-goals

This decision does not sign Loaves or registry assets (RFC 125 does), does not sign source commits, does not define a key-based alternative, and does not decide how the project holds its package names on npm, PyPI and Homebrew or their lookalikes, which is a maintainer action outside the repository.

## Revisit condition

Revisit when Sigstore's trust-root distribution changes, when a channel cannot supply a trusted verification path, or when the project needs to strengthen the convenient HTTPS bootstrap guarantee.

## Provenance

This record derives from the maintainer's decision of 2026-11-07 and clarification of 2026-11-08 that each installation channel must state its initial trust and avoid circular self-verification. It fills the gap DD-0010 left open; it supersedes nothing. RFC 125 is the authority for the registry's attestations and root key.

## References

- DD-0010: Released toolchains for CI and development
- RFC 125: the Loaf registry and baked asset distribution
- Sigstore keyless signing and bundle verification

# Export providers

Status: proposed interface and source review. Candidate repository review began 2026-10-03; vendor retrieval was confirmed and the document reviewed on 2026-10-04. No provider adapter is implemented by this document. No third-party exporter was run, and BoothReady has performed no player tests of these outputs.

## Workflow boundary

Google Drive or a selected local folder feeds a verified local collection. An export provider consumes that collection and produces either an interchange file for DJ software or a complete staged export tree. BoothReady validates a complete export before the existing copy and verification flow transfers it to the exact USB the user confirms.

Cloud download success and a readable audio file do not establish player compatibility. The selected player's partition, filesystem and audio requirements still come from `crates/boothready-core/data/ruleset.json`. Export capability describes what a provider can produce; it cannot override those rules.

A rekordbox XML file is an interchange artifact. It requires an import and export step in rekordbox before this workflow can treat the result as a player library. Device Library (`export.pdb`) and OneLibrary (`exportLibrary.db`, formerly Device Library Plus) are distinct export formats. An adapter must declare each format explicitly. Presence alone is insufficient to establish complete contents, especially while BoothReady's OneLibrary inspection remains limited to presence.

## Candidate providers

The repository links below are pinned to the commits inspected, so later changes require a new review. Project descriptions and hardware reports are upstream claims, with no BoothReady hardware evidence.

| Provider | Input and output | Evidence and limits | Integration status |
|---|---|---|---|
| Manual rekordbox handoff | Locally staged audio, optionally XML, imported into rekordbox; user exports a complete USB library through rekordbox | Vendor documents XML import and Device Library/OneLibrary export. The installed version and requested export settings must be recorded. | First proposed route; no automation or bundled rekordbox |
| [MixxxToRekordbox](https://github.com/bendlas/MixxxToRekordbox/blob/313ba987db13cecf1f837033ede7527f2146fe01/README.md) | Mixxx playlists or crates to rekordbox XML, with optional FFmpeg conversion | README claims metadata, beat grid and hot cue preservation. Requires a subsequent rekordbox export. No player-ready output claimed here. | Adapter unavailable; validate metadata fidelity and review GPL integration first |
| [REX](https://github.com/kimtore/rex/blob/d51cb374acc7df19581100cd230f28c50eea4a4d/README.md) | Mixxx library to legacy PDB plus copied audio | Explicitly lacks waveforms, beat grid and hot cues; reports varying results on unnamed Pioneer devices and failure to import a library on Denon Prime 4. No OneLibrary claim established. | Experimental candidate, blocked on license clarification and validation |
| [fourfour](https://github.com/morizkraemer/fourfour/blob/64091d39642bb071709201a9a74746629098fb0f/README.md) | Local audio plus analysis to PDB and OneLibrary through its format writer | Top-level README reports CDJ-3000 success. The [writer README](https://github.com/morizkraemer/fourfour/blob/64091d39642bb071709201a9a74746629098fb0f/pioneer-usb-writer/README.md) lists broader targets, while requiring analysis supplied by a caller and documenting placeholder color waveforms and album-artist limitations. | Experimental candidate, blocked on license clarification and fixture/player validation |

### Source excerpts

These short excerpts establish the decisions above. No compatibility rules were changed.

- MixxxToRekordbox: “You can then process the files in Rekordbox, export them to your USB drive and delete the temporary folder.” This establishes the handoff requirement.
- REX: “These features are NOT supported yet:” followed by “Waveforms”, “Beat grid”, “Hot Cue”. Required-feature planning must reject that combination.
- fourfour top-level README: “Status: the format library is proven on a CDJ-3000.” This is the project's claim for that model. Its [format notes](https://github.com/morizkraemer/fourfour/blob/64091d39642bb071709201a9a74746629098fb0f/pioneer-usb-writer/reference-code/PIONEER.md), read on 2026-10-04, identify CDJ-3000 firmware 3.19. An independently reproduced BoothReady test remains missing.
- fourfour writer README: “Color waveforms” are “placeholder (solid green)”. A plan requiring meaningful color waveforms must remain blocked.

### License review

[MixxxToRekordbox's LICENSE](https://github.com/bendlas/MixxxToRekordbox/blob/313ba987db13cecf1f837033ede7527f2146fe01/LICENSE) contains GNU GPL version 3. Its integration and redistribution obligations require review before bundling or adapting code.

The recursive repository trees for REX and fourfour at the pinned commits contained no LICENSE or COPYING file, and GitHub's repository license field was null. fourfour's workspace and writer Cargo manifests also declared no license. Record these as **license unresolved**, even though the projects publish source. Do not copy their implementation or add them as dependencies until permission and dependency obligations are established. A legal review still needs to establish the applicable permissions for individual files and dependencies.

### Vendor references

- [rekordbox developer XML documentation](https://rekordbox.com/en/support/developer/) describes importing a generated XML library.
- [rekordbox OneLibrary FAQ](https://rekordbox.com/en/support/faq/onelibrary-7/) describes both database formats and conversion through rekordbox.

Both vendor pages were retrieved directly over HTTPS after the browser fetch timed out. The developer page states: “You can now display your rekordbox playlists in the [Bridge] pane by importing playlist information from an XML file.” The FAQ states: “You can convert the traditional Device Library in the USB storage device to the OneLibrary format with rekordbox for Mac/Windows.” These establish the interchange and conversion paths without verifying a particular player. The candidate repository READMEs and license checks above were read directly at pinned commits through GitHub's public API/raw content.

## Proposed contract

Keep the contract versioned and independent of a specific exporter. Names below are design fields rather than an implemented Rust API.

| Value | Required information |
|---|---|
| `CollectionInput` | Completed staging manifest identity and schema version; content revision; file IDs with relative paths, sizes and BLAKE3 hashes; a local-only root handle supplied at execution |
| `LibraryMetadata` | Optional playlist membership, cue data and analysis, each with explicit availability and provenance; plain audio staging does not imply a Mixxx library exists |
| `TargetRequest` | Concrete ruleset device IDs, ruleset version, known firmware or explicit unknown, requested library formats and required DJ features |
| `ProviderDescriptor` | Provider ID and pinned version, accepted input kinds, possible artifact kinds, feature support states, license status, adapter availability and evidence references |
| `ExportPlan` | Input revision, resolved requirements for every target, missing metadata/features, required handoff, blocking reasons and allowed next action |
| `ExportArtifact` | Artifact kind, provider version, source revision, file manifest, generation status and validation results; keep the local root outside shareable reports |
| `TargetEvidence` | Exact model/firmware scope, provider version, artifact digest, evidence source, date and tested operations; distinguish upstream report from BoothReady fixture or player test |

The parallel staging implementation proposes `boothready_core::staging::Collection` version 1 with `kind`, a private `source` path, `revision`, and ordered `files`. Each file record contains relative `path`, `bytes` and `blake3`. Its directory holds `collection.json`, `files/` and a completion record in `complete.json`. An adapter boundary should project only `revision` and file records into `CollectionInput`, with the `files/` root supplied separately; it must discard `source`. Reverify the snapshot at consumption because a completion marker cannot establish that files remained unchanged. This mapping awaits integration with that separate PR.

Artifact kinds are `rekordbox_xml`, `device_library_tree`, `onelibrary_tree` and `dual_library_tree`. A tree includes its referenced audio and applicable analysis/artwork, so a standalone database file cannot count as a complete export. Unknown formats fail planning rather than falling through to folder copying.

Feature states are `supported`, `unsupported` and `unknown`; a separate evidence field records provenance. Required unknown features block automatic export. Successful fixture validation does not promote an upstream hardware report to a BoothReady player test. A provider's broad README compatibility list leaves each untested target's hardware state unknown.

Do not feed local absolute paths, Google Drive credentials, signed download URLs or raw diagnostics into shared capability records. Local paths and playlist names can reveal personal information, so runtime collection manifests and export reports remain local and outside Git. Published fixtures use synthetic audio and metadata.

## Execution and verification gates

1. Staging must finish and verify every selected file. Reject changed inputs, placeholders or missing files; an interrupted collection cannot become an export input. If an adapter needs Mixxx metadata, validate a separately selected snapshot against the staged files before planning.
2. Resolve each target through the existing ruleset, then intersect its requirements with the provider's declared output and features. A missing target profile or unsupported required library format blocks the plan. XML produces `handoff_required` even when the XML validates.
3. Any future automated provider writes to a fresh local staging directory and receives no physical device ID or USB mount path. A path parameter alone is not a sandbox: review its writes and enforce isolation before enabling a third-party executable. Manual rekordbox export uses the existing user-controlled handoff and drive confirmation boundary.
4. Validate database structure and all referenced relative paths, reject traversal and symlinks escaping the artifact, then verify track coverage and audio hashes. Compare requested cues/playlists against supported readers. Report OneLibrary semantic validation as unavailable while its reader cannot establish those contents.
5. Mark a complete local artifact eligible for the existing copy planner only when required validation has passed. Keep an incomplete marker through interruption. Never splice selected rows into an existing DJ database; a smaller selection requires regeneration of the entire export.
6. USB writes still require the user's immediate confirmation of the exact device, model and size. Full destination read-back belongs to the existing verification flow. Player testing is a separate record with explicit model, firmware and observations.

## First adapter acceptance cases

Use synthetic tracks and playlists for these cases. They are planned tests for a later implementation, not test results from this review.

| Case | Required outcome |
|---|---|
| XML supplied where a Device Library is required | Handoff required; USB copy remains unavailable |
| Legacy-only provider with a OneLibrary requirement | Plan blocked with the missing format |
| REX with hot cues required | Plan blocked with unsupported feature |
| fourfour with color waveforms required | Plan blocked until the placeholder limitation is resolved and validated |
| Local audio without a Mixxx database | Mixxx-dependent provider requires metadata input |
| File changes after staging validation | Reject the obsolete collection revision |
| Missing referenced audio or interrupted export | Artifact remains incomplete |
| Unknown firmware or only upstream player report | Preserve unknown local hardware verification |
| Artifact passes fixture tests | Record fixture evidence without claiming player compatibility |
| Existing destination has another library | Existing copy planner rejects mixing libraries |

The next implementation should add a pure planner for this contract and the failure cases above before enabling a provider executable. The manual rekordbox route can use the same handoff states while licensing and compatibility evidence for experimental providers are reviewed.

# Retained path field at a generic Rust boundary

`generic_path_arg.incn` passes `incan check` but native baking fails with Rust E0382: `command.arg(path.0)` partially moves the path before its later `joinpath` call. The command is never executed; compilation alone demonstrates the failure. The reduced program was checked and baked using the retained stage-3 development compiler and the corrected prepared SDK.

The selected-test orchestration uses an ordinary Incan workaround: retain the path text first, construct the separate Path value, then pass the text to Command. The baked orchestration executed the complete store root and its unchanged repeat successfully. No Rust implementation was added for this workaround.

Duplicate searches covered E0382, partially moved values and generic interop. [#845](https://github.com/encero-systems/incan/issues/845) is closed and concerns a whole Rust file handle passed to an AsFd API. This repro concerns a field of an Incan newtype passed to AsRef-style Command arguments. [#1793](https://github.com/encero-systems/incan/issues/1793) is closed and concerns static dictionary argument temporaries. Neither reproduces this exact boundary. The interop matrix owner is [#872](https://github.com/encero-systems/incan/issues/872); a separate bug draft is retained in the recovery evidence directory and has not been published.

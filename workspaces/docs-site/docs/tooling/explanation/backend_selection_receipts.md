# Native compilation and output identity

Dev.8 has one native compilation route. Checked Body IR reaches Incan lowering and the driver's Rust MIR adapter, then the pinned Rust compiler produces native artifacts. A program does not select a backend.

Oven retains executables while their observed inputs and output identity are unchanged. The reuse record belongs to that compilation unit and its installed compiler closure. A changed source, selected feature, dependency or artifact invalidates the corresponding reuse.

Earlier releases exposed backend-selection receipts and generated-Rust inspection commands. Those are not interfaces of the dev.8 CLI. Use `incan describe-unit PATH` for selected compilation inputs, `incan check PATH --format json` for diagnostics and `OVEN_TRACE=-` for executable reuse decisions.

[Projects in dev.8](projects_today.md) explains ownership; the [CLI reference](../reference/cli_reference.md) specifies commands.

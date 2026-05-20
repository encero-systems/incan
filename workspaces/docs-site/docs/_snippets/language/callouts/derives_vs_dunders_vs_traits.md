!!! info "Derives vs dunders vs traits (pick the mechanism)"
    - **`@derive(...)`**: ask the compiler to generate a **default/structural** implementation for a built-in behavior (usually field-based).
    - **Dunder methods** (e.g. `__eq__`, `__lt__`, `__hash__`, `__str__`): provide a **custom implementation** of that behavior. If you define the dunder, you **do not** need to also `@derive` the corresponding trait.
    - **Custom traits** (`trait ...` + `with TraitName`): define **domain capabilities**. Implement required methods (and override defaults) by writing normal methods.
    - **`Debug` is special**: `{:?}` is always compiler-generated; `__repr__` is not user-overridable (use `__str__` + `{}` for custom output).

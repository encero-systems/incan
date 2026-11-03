//! Alias, partial-preset, newtype-cycle and method alias declaration validation.

use super::*;

impl TypeChecker {
    /// Validate alias and partial relationships that need declaration-level context before symbol collection flattens
    /// aliases and projected callable presets.
    pub(super) fn validate_alias_declarations(&mut self, program: &Program) {
        let mut public_decls = HashMap::new();
        let mut alias_targets = HashMap::new();
        let mut partial_targets = HashMap::new();
        let mut newtype_names = HashSet::new();
        let mut newtype_underlyings = HashMap::new();

        for decl in &program.declarations {
            if let Some(name) = declaration_name(decl) {
                public_decls.insert(name.to_string(), is_public_decl(decl));
            }
            if let Declaration::Newtype(nt) = &decl.node {
                newtype_names.insert(nt.name.clone());
            }
        }

        for decl in &program.declarations {
            match &decl.node {
                Declaration::Alias(_) | Declaration::Partial(_) => {
                    self.collect_checked_projection_targets(
                        decl,
                        &public_decls,
                        &mut alias_targets,
                        &mut partial_targets,
                    );
                }
                Declaration::Model(model) => {
                    self.validate_member_name_collisions(
                        &model.name,
                        &model.fields,
                        &model.method_aliases,
                        &model.method_partials,
                        &model.properties,
                        &model.methods,
                    );
                    self.validate_method_alias_declarations(&model.name, &model.method_aliases, &model.methods);
                    self.validate_method_partial_declarations(
                        &model.name,
                        &model.method_aliases,
                        &model.method_partials,
                        &model.methods,
                    );
                }
                Declaration::Class(class) => {
                    self.validate_member_name_collisions(
                        &class.name,
                        &class.fields,
                        &class.method_aliases,
                        &class.method_partials,
                        &class.properties,
                        &class.methods,
                    );
                    self.validate_method_alias_declarations(&class.name, &class.method_aliases, &class.methods);
                    self.validate_method_partial_declarations(
                        &class.name,
                        &class.method_aliases,
                        &class.method_partials,
                        &class.methods,
                    );
                }
                Declaration::Trait(tr) => {
                    self.validate_member_name_collisions(
                        &tr.name,
                        &[],
                        &tr.method_aliases,
                        &tr.method_partials,
                        &tr.properties,
                        &tr.methods,
                    );
                    self.validate_method_alias_declarations(&tr.name, &tr.method_aliases, &tr.methods);
                    self.validate_method_partial_declarations(
                        &tr.name,
                        &tr.method_aliases,
                        &tr.method_partials,
                        &tr.methods,
                    );
                }
                Declaration::Newtype(nt) => {
                    self.validate_member_name_collisions(
                        &nt.name,
                        &[],
                        &nt.method_aliases,
                        &nt.method_partials,
                        &[],
                        &nt.methods,
                    );
                    self.validate_method_alias_declarations(&nt.name, &nt.method_aliases, &nt.methods);
                    self.validate_method_partial_declarations(
                        &nt.name,
                        &nt.method_aliases,
                        &nt.method_partials,
                        &nt.methods,
                    );
                    if let Type::Simple(target) = &nt.underlying.node {
                        newtype_underlyings.insert(nt.name.clone(), (target.clone(), nt.underlying.span));
                    }
                }
                Declaration::Enum(en) => {
                    self.validate_enum_member_name_collisions(&en.name, &en.variants, &en.variant_aliases, &en.methods);
                }
                _ => {}
            }
        }

        self.validate_top_level_alias_cycles(&alias_targets);
        self.validate_top_level_partial_cycles(&alias_targets, &partial_targets);
        self.validate_newtype_underlying_cycles(&newtype_underlyings, &newtype_names);
    }

    /// Collect local alias or partial targets and reject public projections of private declarations before cycle
    /// checks.
    pub(super) fn collect_checked_projection_targets(
        &mut self,
        decl: &Spanned<Declaration>,
        public_decls: &HashMap<String, bool>,
        alias_targets: &mut HashMap<String, (String, Span)>,
        partial_targets: &mut HashMap<String, (String, Span)>,
    ) {
        match &decl.node {
            Declaration::Alias(alias) => {
                if let [target] = alias.target.segments.as_slice() {
                    alias_targets.insert(alias.name.clone(), (target.clone(), decl.span));
                    if matches!(alias.visibility, Visibility::Public)
                        && public_decls.get(target).is_some_and(|is_public| !*is_public)
                    {
                        self.errors.push(CompileError::type_error(
                            format!(
                                "Public alias '{}' targets private symbol '{}'",
                                alias.name,
                                alias.target.segments.join(".")
                            ),
                            decl.span,
                        ));
                    }
                }
            }
            Declaration::Partial(partial) => {
                if let [target] = partial.target.segments.as_slice() {
                    partial_targets.insert(partial.name.clone(), (target.clone(), decl.span));
                    if matches!(partial.visibility, Visibility::Public)
                        && public_decls.get(target).is_some_and(|is_public| !*is_public)
                    {
                        self.errors.push(CompileError::type_error(
                            format!(
                                "Public partial '{}' targets private symbol '{}'",
                                partial.name,
                                partial.target.segments.join(".")
                            ),
                            decl.span,
                        ));
                    }
                }
                if matches!(partial.visibility, Visibility::Public) {
                    self.validate_public_partial_presets_do_not_reference_private_symbols(
                        &partial.name,
                        &partial.args,
                        public_decls,
                    );
                }
            }
            _ => {}
        }
    }

    /// Reject direct and indirect cycles between top-level aliases.
    pub(super) fn validate_top_level_alias_cycles(&mut self, alias_targets: &HashMap<String, (String, Span)>) {
        let mut reported = HashSet::new();
        for (name, (_, span)) in alias_targets {
            let mut path = Vec::new();
            let mut current = name.clone();
            while let Some((target, _)) = alias_targets.get(&current) {
                if let Some(cycle_start) = path.iter().position(|seen| seen == &current) {
                    let mut cycle = path[cycle_start..].to_vec();
                    cycle.push(current.clone());
                    let key = cycle.join(" -> ");
                    if reported.insert(key.clone()) {
                        self.errors
                            .push(CompileError::type_error(format!("Alias cycle detected: {key}"), *span));
                    }
                    break;
                }
                path.push(current);
                current = target.clone();
            }
        }
    }

    /// Reject direct and indirect cycles in top-level partial declarations, including cycles that route through
    /// aliases.
    pub(super) fn validate_top_level_partial_cycles(
        &mut self,
        alias_targets: &HashMap<String, (String, Span)>,
        partial_targets: &HashMap<String, (String, Span)>,
    ) {
        let mut edges: HashMap<String, (String, Span)> = alias_targets.clone();
        edges.extend(partial_targets.clone());

        let mut reported = HashSet::new();
        for (name, (_, span)) in partial_targets {
            let mut path = Vec::new();
            let mut current = name.clone();
            while let Some((target, _)) = edges.get(&current) {
                if let Some(cycle_start) = path.iter().position(|seen| seen == &current) {
                    let mut cycle = path[cycle_start..].to_vec();
                    cycle.push(current.clone());
                    let key = cycle.join(" -> ");
                    if cycle.iter().any(|segment| partial_targets.contains_key(segment)) && reported.insert(key.clone())
                    {
                        self.errors.push(CompileError::type_error(
                            format!("Partial cycle detected: {key}"),
                            *span,
                        ));
                    }
                    break;
                }
                path.push(current);
                current = target.clone();
            }
        }
    }

    /// Public partial provenance must remain checkable for consumers without private module state.
    pub(super) fn validate_public_partial_presets_do_not_reference_private_symbols(
        &mut self,
        partial_name: &str,
        args: &[PartialArg],
        public_decls: &HashMap<String, bool>,
    ) {
        for arg in args {
            self.validate_public_partial_preset_expr(partial_name, &arg.name, &arg.value, public_decls);
        }
    }

    /// Recursively reject private roots used by public partial preset values.
    pub(super) fn validate_public_partial_preset_expr(
        &mut self,
        partial_name: &str,
        preset_name: &str,
        expr: &Spanned<Expr>,
        public_decls: &HashMap<String, bool>,
    ) {
        if let Some(root) = preset_private_root(expr, public_decls) {
            self.errors.push(CompileError::type_error(
                format!("Public partial '{partial_name}' preset '{preset_name}' references private symbol '{root}'"),
                expr.span,
            ));
        }

        match &expr.node {
            Expr::Paren(inner) => {
                self.validate_public_partial_preset_expr(partial_name, preset_name, inner, public_decls)
            }
            Expr::List(entries) => {
                for entry in entries {
                    match entry {
                        ListEntry::Element(value) | ListEntry::Spread(value) => {
                            self.validate_public_partial_preset_expr(partial_name, preset_name, value, public_decls);
                        }
                    }
                }
            }
            Expr::Dict(entries) => {
                for entry in entries {
                    match entry {
                        DictEntry::Pair(key, value) => {
                            self.validate_public_partial_preset_expr(partial_name, preset_name, key, public_decls);
                            self.validate_public_partial_preset_expr(partial_name, preset_name, value, public_decls);
                        }
                        DictEntry::Spread(value) => {
                            self.validate_public_partial_preset_expr(partial_name, preset_name, value, public_decls);
                        }
                    }
                }
            }
            Expr::Call(_, _, args) => {
                for arg in args {
                    match arg {
                        CallArg::Positional(value)
                        | CallArg::Named(_, value)
                        | CallArg::PositionalUnpack(value)
                        | CallArg::KeywordUnpack(value) => {
                            self.validate_public_partial_preset_expr(partial_name, preset_name, value, public_decls)
                        }
                    }
                }
            }
            Expr::Constructor(_, args) => {
                for arg in args {
                    match arg {
                        CallArg::Positional(value)
                        | CallArg::Named(_, value)
                        | CallArg::PositionalUnpack(value)
                        | CallArg::KeywordUnpack(value) => {
                            self.validate_public_partial_preset_expr(partial_name, preset_name, value, public_decls)
                        }
                    }
                }
            }
            _ => {}
        }
    }

    /// Reject direct and indirect cycles in newtype underlying declarations before coercion planning.
    pub(super) fn validate_newtype_underlying_cycles(
        &mut self,
        newtype_underlyings: &HashMap<String, (String, Span)>,
        newtype_names: &HashSet<String>,
    ) {
        let mut reported = HashSet::new();
        for (name, (_, span)) in newtype_underlyings {
            let mut path = Vec::new();
            let mut current = name.clone();
            while let Some((target, _)) = newtype_underlyings.get(&current) {
                if !newtype_names.contains(target) {
                    break;
                }
                if let Some(cycle_start) = path.iter().position(|seen| seen == &current) {
                    let mut cycle = path[cycle_start..].to_vec();
                    cycle.push(current.clone());
                    let key = cycle.join(" -> ");
                    if reported.insert(key.clone()) {
                        self.errors.push(errors::newtype_coercion_cycle(&key, *span));
                    }
                    break;
                }
                path.push(current);
                current = target.clone();
            }
        }
    }

    /// Validate same-type method alias names and targets for one method-bearing declaration.
    pub(super) fn validate_method_alias_declarations(
        &mut self,
        owner: &str,
        aliases: &[Spanned<MethodAliasDecl>],
        methods: &[Spanned<MethodDecl>],
    ) {
        let method_names: HashSet<&str> = methods
            .iter()
            .filter(|method| self.member_binding_is_active(method.span))
            .map(|method| method.node.name.as_str())
            .collect();
        let mut alias_targets = HashMap::new();

        for alias in aliases {
            if !self.member_binding_is_active(alias.span) {
                continue;
            }
            if !method_names.contains(alias.node.target.as_str()) {
                self.errors.push(CompileError::type_error(
                    format!(
                        "Method alias '{}.{}' targets unknown method '{}'",
                        owner, alias.node.name, alias.node.target
                    ),
                    alias.span,
                ));
            }
            alias_targets.insert(alias.node.name.clone(), (alias.node.target.clone(), alias.span));
        }

        self.validate_method_alias_cycles(owner, &alias_targets);
    }

    /// Reject direct and indirect cycles between method aliases on one owner type.
    pub(super) fn validate_method_alias_cycles(
        &mut self,
        owner: &str,
        alias_targets: &HashMap<String, (String, Span)>,
    ) {
        let mut reported = HashSet::new();
        for (name, (_, span)) in alias_targets {
            let mut path = Vec::new();
            let mut current = name.clone();
            while let Some((target, _)) = alias_targets.get(&current) {
                if let Some(cycle_start) = path.iter().position(|seen| seen == &current) {
                    let mut cycle = path[cycle_start..].to_vec();
                    cycle.push(current.clone());
                    let key = cycle.join(" -> ");
                    if reported.insert(key.clone()) {
                        self.errors.push(CompileError::type_error(
                            format!("Method alias cycle detected on '{owner}': {key}"),
                            *span,
                        ));
                    }
                    break;
                }
                path.push(current);
                current = target.clone();
            }
        }
    }

    /// Validate same-type method partial names and targets before collection turns them into method metadata.
    pub(super) fn validate_method_partial_declarations(
        &mut self,
        owner: &str,
        aliases: &[Spanned<MethodAliasDecl>],
        partials: &[Spanned<MethodPartialDecl>],
        methods: &[Spanned<MethodDecl>],
    ) {
        let method_names: HashSet<&str> = methods
            .iter()
            .filter(|method| self.member_binding_is_active(method.span))
            .map(|method| method.node.name.as_str())
            .collect();
        let alias_names: HashSet<&str> = aliases
            .iter()
            .filter(|alias| self.member_binding_is_active(alias.span))
            .map(|alias| alias.node.name.as_str())
            .collect();

        for partial in partials {
            if !self.member_binding_is_active(partial.span) {
                continue;
            }
            if !method_names.contains(partial.node.target.as_str())
                && !alias_names.contains(partial.node.target.as_str())
            {
                self.errors.push(CompileError::type_error(
                    format!(
                        "Method partial '{}.{}' targets unknown method '{}'",
                        owner, partial.node.name, partial.node.target
                    ),
                    partial.span,
                ));
            }
        }
    }
}

/// Return the private top-level root named by a declaration-safe preset expression, if any.
pub(super) fn preset_private_root(expr: &Spanned<Expr>, public_decls: &HashMap<String, bool>) -> Option<String> {
    match &expr.node {
        Expr::Ident(name) => public_decls
            .get(name)
            .is_some_and(|is_public| !*is_public)
            .then(|| name.clone()),
        Expr::Field(base, _) => preset_private_root(base, public_decls),
        Expr::Paren(inner) => preset_private_root(inner, public_decls),
        Expr::Call(callee, _, _) => preset_private_root(callee, public_decls),
        Expr::Constructor(name, _) => public_decls
            .get(name)
            .is_some_and(|is_public| !*is_public)
            .then(|| name.clone()),
        _ => None,
    }
}

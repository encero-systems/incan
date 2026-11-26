//! Keyword registrations, activations, specs and placements.

use super::*;

/// A group of keywords that share one activation rule.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct KeywordRegistration {
    /// Activation rule for this keyword group.
    pub activation: KeywordActivation,
    /// Keywords introduced by this group.
    #[cfg_attr(feature = "serde", serde(default))]
    pub keywords: Vec<KeywordSpec>,
    /// Decorators valid on blocks introduced by these keywords.
    #[cfg_attr(feature = "serde", serde(default))]
    pub valid_decorators: Vec<String>,
}

impl KeywordRegistration {
    /// Create an empty keyword registration for one activation rule.
    #[must_use]
    pub fn new(activation: KeywordActivation) -> Self {
        Self {
            activation,
            keywords: Vec::new(),
            valid_decorators: Vec::new(),
        }
    }

    /// Create an import-activated keyword registration.
    #[must_use]
    pub fn on_import(namespace: &str) -> Self {
        Self::new(KeywordActivation::on_import(namespace))
    }

    /// Create an always-active keyword registration.
    #[must_use]
    pub fn always_on() -> Self {
        Self::new(KeywordActivation::Always)
    }

    /// Add one keyword to the registration.
    #[must_use]
    pub fn with_keyword(mut self, keyword: KeywordSpec) -> Self {
        self.keywords.push(keyword);
        self
    }

    /// Add multiple keywords to the registration.
    #[must_use]
    pub fn with_keywords<I>(mut self, keywords: I) -> Self
    where
        I: IntoIterator<Item = KeywordSpec>,
    {
        self.keywords.extend(keywords);
        self
    }

    /// Add one valid decorator to the registration.
    #[must_use]
    pub fn with_valid_decorator(mut self, decorator: impl Into<String>) -> Self {
        self.valid_decorators.push(decorator.into());
        self
    }

    /// Add multiple valid decorators to the registration.
    #[must_use]
    pub fn with_valid_decorators<I, S>(mut self, decorators: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.valid_decorators.extend(decorators.into_iter().map(Into::into));
        self
    }
}

/// Activation rule for a keyword group.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[non_exhaustive]
pub enum KeywordActivation {
    /// Always active in every file.
    #[default]
    Always,
    /// Activated when a matching import path is used.
    ///
    /// The namespace should use the library-facing import spelling, for example `mydsl.routes`.
    OnImport { namespace: String },
}

impl KeywordActivation {
    /// Create an import-activated keyword rule.
    #[must_use]
    pub fn on_import(namespace: &str) -> Self {
        Self::OnImport {
            namespace: namespace.to_string(),
        }
    }
}

/// Specification for a single keyword.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct KeywordSpec {
    /// Primary keyword token as written by the user.
    pub name: String,
    /// Parser surface occupied by the keyword.
    pub surface_kind: KeywordSurfaceKind,
    /// Additional tokens for compound keyword spellings.
    #[cfg_attr(feature = "serde", serde(default))]
    pub compound_tokens: Vec<String>,
    /// Where the keyword is valid.
    pub placement: KeywordPlacement,
}

impl KeywordSpec {
    /// Create a simple top-level keyword.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use incan_vocab::{KeywordPlacement, KeywordSpec, KeywordSurfaceKind};
    ///
    /// let spec = KeywordSpec::new("route", KeywordSurfaceKind::BlockDeclaration);
    /// assert_eq!(spec.name, "route");
    /// assert_eq!(spec.surface_kind, KeywordSurfaceKind::BlockDeclaration);
    /// assert_eq!(spec.placement, KeywordPlacement::TopLevel);
    /// assert!(spec.compound_tokens.is_empty());
    /// ```
    #[must_use]
    pub fn new(name: &str, surface_kind: KeywordSurfaceKind) -> Self {
        Self {
            name: name.to_string(),
            surface_kind,
            compound_tokens: Vec::new(),
            placement: KeywordPlacement::TopLevel,
        }
    }

    /// Create a top-level DSL block declaration keyword.
    #[must_use]
    pub fn block(name: &str) -> Self {
        Self::new(name, KeywordSurfaceKind::BlockDeclaration)
    }

    /// Create a block-context keyword scoped to one parent block.
    #[must_use]
    pub fn block_context(name: &str, parent_keyword: &str) -> Self {
        Self::new(name, KeywordSurfaceKind::BlockContextKeyword).in_block(parent_keyword)
    }

    /// Create a sub-block keyword scoped to one parent block.
    #[must_use]
    pub fn sub_block(name: &str, parent_keyword: &str) -> Self {
        Self::new(name, KeywordSurfaceKind::SubBlock).in_block(parent_keyword)
    }

    /// Replace the compound-token tail for this keyword.
    #[must_use]
    pub fn with_compound_tokens<I, S>(mut self, compound_tokens: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.compound_tokens = compound_tokens.into_iter().map(Into::into).collect();
        self
    }

    /// Set the placement rule for this keyword.
    #[must_use]
    pub fn with_placement(mut self, placement: KeywordPlacement) -> Self {
        self.placement = placement;
        self
    }

    /// Scope this keyword to one parent block.
    #[must_use]
    pub fn in_block(self, parent_keyword: &str) -> Self {
        self.with_placement(KeywordPlacement::in_block([parent_keyword]))
    }

    /// Scope this keyword to one of several parent blocks.
    #[must_use]
    pub fn in_blocks<I, S>(self, parent_keywords: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.with_placement(KeywordPlacement::in_block(parent_keywords))
    }
}

/// Where a keyword is valid.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[non_exhaustive]
pub enum KeywordPlacement {
    /// Valid in ordinary statement/declaration positions.
    #[default]
    TopLevel,
    /// Valid only inside one of the provided parent blocks.
    ///
    /// The strings here refer to registered parent keyword names.
    InBlock(Vec<String>),
}

impl KeywordPlacement {
    /// Create a placement rule that is valid inside the provided parent blocks.
    #[must_use]
    pub fn in_block<I, S>(parent_keywords: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Self::InBlock(parent_keywords.into_iter().map(Into::into).collect())
    }
}

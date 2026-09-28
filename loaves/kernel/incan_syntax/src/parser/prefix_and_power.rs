/// Prefix-operator and power parsing: the two levels of the expression ladder between multiplicative expressions and
/// postfix expressions.
///
/// `**` binds tighter than a prefix `-` or `~` on its left and looser than one on its right, and a prefix soft keyword
/// (`await`) binds tighter than `**`.
impl<'a> Parser<'a> {
    /// Parse prefix `-` and RFC 028 bitwise inversion `~`.
    ///
    /// A prefix operator's operand is itself a prefix expression (`- -x`), and a `**` inside it binds first: `-x ** 2`
    /// is `-(x ** 2)` and `~x ** 2` is `~(x ** 2)`.
    fn unary(&mut self) -> Result<Spanned<Expr>, CompileError> {
        let op = if self.match_token(&TokenKind::Operator(OperatorId::Minus)) {
            UnaryOp::Neg
        } else if self.match_token(&TokenKind::Operator(OperatorId::Tilde)) {
            UnaryOp::Invert
        } else {
            return self.power();
        };
        let start = self.tokens[self.pos - 1].span.start;
        let expr = self.unary()?;
        let span = Span::new(start, expr.span.end);
        Ok(Spanned::new(Expr::Unary(op, Box::new(expr)), span))
    }

    /// Parse `**`: right-associative, tighter than a prefix operator on its left, looser than one on its right.
    ///
    /// The exponent is read at the prefix level, so `2 ** -1` is `2 ** (-1)` and `2 ** 3 ** 2` is `2 ** (3 ** 2)`.
    fn power(&mut self) -> Result<Spanned<Expr>, CompileError> {
        let mut left = self.power_base()?;

        if self.match_token(&TokenKind::Operator(OperatorId::StarStar)) {
            let right = self.unary()?;
            let span = left.span.merge(right.span);
            left = Spanned::new(Expr::Binary(Box::new(left), BinaryOp::Pow, Box::new(right)), span);
        }

        Ok(left)
    }

    /// Parse the base of a power: a postfix expression, or a prefix soft keyword (`await`) applied to one.
    ///
    /// `await` binds tighter than `**`, so `await x ** 2` is `(await x) ** 2`. An operand that starts with `-` or `~`
    /// is read as a whole prefix expression (`await -x`).
    fn power_base(&mut self) -> Result<Spanned<Expr>, CompileError> {
        let Some(id) = self.current_surface_keyword(KeywordSurfaceKind::PrefixExpression) else {
            return self.postfix();
        };
        self.advance();
        let start = self.tokens[self.pos - 1].span.start;
        let expr = if self.check_op(OperatorId::Minus) || self.check_op(OperatorId::Tilde) {
            self.unary()?
        } else {
            self.power_base()?
        };
        let span = Span::new(start, expr.span.end);
        Ok(Spanned::new(
            Expr::Surface(Box::new(SurfaceExpr {
                key: SurfaceFeatureKey::SoftKeyword(id),
                payload: SurfaceExprPayload::PrefixUnary(Box::new(expr)),
            })),
            span,
        ))
    }
}

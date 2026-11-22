# encoding: utf-8
# frozen_string_literal: true

# Rewrite the Rust spelling of borrowed types in Incan type annotations to the Incan one (RFC 164):
# `&T` becomes `Borrowed[T]` and `&mut T` becomes `BorrowedMut[T]`. Only annotations are touched: def parameters,
# def return types, and `name: Type` bindings, fields, statics and constants at the start of a statement, plus
# `type Name = ...` aliases. Expressions (`f(&x)`, `[&a, &b]`, `y = &mut z`) are never changed.
#
# Usage: ruby borrowed_spelling.rb [--check] PATH...
#   Each PATH is a file or a directory searched for *.incn, in sorted order. Without --check the files are rewritten
#   in place; with --check nothing is written. Either way each changed file is listed with its count, then a total.
#   The rewrite is deterministic and idempotent: a second run changes nothing.

Token = Struct.new(:kind, :text, :from, :to, :line_start, :depth)

# Split Incan source into tokens, keeping byte offsets; strings and comments are single opaque tokens.
def tokens(source)
  out = []
  i = 0
  depth = 0
  line_start = true
  n = source.bytesize
  s = source.b
  while i < n
    c = s[i]
    if c == "\n"
      out << Token.new(:newline, c, i, i + 1, false, depth)
      i += 1
      line_start = true
      next
    end
    if c == " " || c == "\t" || c == "\r"
      i += 1
      next
    end
    if c == "#"
      j = s.index("\n", i) || n
      i = j
      next
    end
    start_of_line = line_start
    line_start = false
    if (m = s[i..].match(/\A([rRbBfF]{0,2})("""|'''|"|')/n))
      quote = m[2]
      j = i + m[0].bytesize
      if quote.bytesize == 3
        k = s.index(quote, j) || n
        j = k + 3
      else
        while j < n && s[j] != quote && s[j] != "\n"
          j += (s[j] == "\\" ? 2 : 1)
        end
        j += 1
      end
      out << Token.new(:string, s[i...j], i, j, start_of_line, depth)
      i = j
      next
    end
    if c =~ /[A-Za-z_]/n
      j = i
      j += 1 while j < n && s[j] =~ /[A-Za-z_0-9]/n
      out << Token.new(:name, s[i...j], i, j, start_of_line, depth)
      i = j
      next
    end
    if c =~ /[0-9]/n
      j = i
      j += 1 while j < n && s[j] =~ /[A-Za-z_0-9.]/n
      out << Token.new(:number, s[i...j], i, j, start_of_line, depth)
      i = j
      next
    end
    two = s[i, 2]
    if ["->", "=>", "==", "!=", "<=", ">=", "+=", "-=", "*=", "/=", "::", "**"].include?(two)
      out << Token.new(:op, two, i, i + 2, start_of_line, depth)
      i += 2
      next
    end
    depth -= 1 if [")", "]", "}"].include?(c)
    out << Token.new(:op, c, i, i + 1, start_of_line, depth)
    depth += 1 if ["(", "[", "{"].include?(c)
    i += 1
  end
  out
end

# One rewrite of an annotation's `&` type: the byte range it covers and its replacement text.
Edit = Struct.new(:from, :to, :text)

class TypeParser
  attr_reader :edits

  def initialize(toks, source)
    @t = toks
    @s = source.b
    @edits = []
  end

  # Parse one type expression at token index i; return the index after it, recording `&` rewrites. Returns nil when
  # the tokens do not form a type, so the caller leaves them alone.
  def type_at(i)
    tok = @t[i]
    return nil unless tok
    if tok.kind == :op && tok.text == "&"
      j = i + 1
      mutable = @t[j] && @t[j].kind == :name && @t[j].text == "mut"
      j += 1 if mutable
      inner_start = j
      after = type_at(j)
      return nil unless after
      inner_from = @t[inner_start].from
      inner_to = @t[after - 1].to
      @edits << [tok.from, inner_from, inner_to, mutable]
      return after
    end
    if tok.kind == :op && tok.text == "("
      return group_at(i, ")")
    end
    if tok.kind == :op && tok.text == "["
      return group_at(i, "]")
    end
    if tok.kind == :string || tok.kind == :number
      return i + 1
    end
    return nil unless tok.kind == :name
    j = i + 1
    while @t[j] && @t[j].kind == :op && @t[j].text == "." && @t[j + 1] && @t[j + 1].kind == :name
      j += 2
    end
    if @t[j] && @t[j].kind == :op && @t[j].text == "[" && @t[j].from == @t[j - 1].to
      j = group_at(j, "]")
      return nil unless j
    end
    j
  end

  # A bracketed, comma-separated list of types; returns the index after the closing bracket.
  def group_at(i, close)
    j = i + 1
    return j + 1 if @t[j] && @t[j].kind == :op && @t[j].text == close
    loop do
      j = type_at(j)
      return nil unless j
      tok = @t[j]
      return nil unless tok
      return j + 1 if tok.kind == :op && tok.text == close
      return nil unless tok.kind == :op && tok.text == ","
      j += 1
      return j + 1 if @t[j] && @t[j].kind == :op && @t[j].text == close
    end
  end
end

STATEMENT_WORDS = %w[pub mut let const static].freeze

# The token indices where an annotation's type starts: after a def parameter's `:`, after `->`, after the `:` of a
# statement-level `name: Type`, and after `=` of a `type Name = ...` alias.
def annotation_starts(toks)
  starts = []
  i = 0
  while i < toks.size
    tok = toks[i]
    if tok.kind == :name && tok.text == "def"
      j = i + 1
      j += 1 if toks[j] && toks[j].kind == :name
      if toks[j] && toks[j].kind == :op && toks[j].text == "["
        depth = toks[j].depth
        j += 1
        j += 1 while toks[j] && !(toks[j].kind == :op && toks[j].text == "]" && toks[j].depth == depth)
        j += 1
      end
      if toks[j] && toks[j].kind == :op && toks[j].text == "("
        list_depth = toks[j].depth + 1
        k = j + 1
        expect_name = true
        while toks[k] && !(toks[k].kind == :op && toks[k].text == ")" && toks[k].depth == list_depth - 1)
          t = toks[k]
          if t.depth == list_depth && t.kind == :op && t.text == ","
            expect_name = true
          elsif expect_name && t.depth == list_depth && t.kind == :name && t.text != "mut" &&
                toks[k + 1] && toks[k + 1].kind == :op && toks[k + 1].text == ":"
            starts << k + 2
            expect_name = false
          elsif t.kind != :newline && !(t.kind == :name && t.text == "mut") && !(t.kind == :op && t.text == "*")
            expect_name = false if t.depth == list_depth
          end
          k += 1
        end
        if toks[k + 1] && toks[k + 1].kind == :op && toks[k + 1].text == "->"
          starts << k + 2
        end
        i = k + 1
        next
      end
    elsif tok.line_start && tok.depth.zero? && tok.kind == :name
      j = i
      j += 1 while toks[j] && toks[j].kind == :name && STATEMENT_WORDS.include?(toks[j].text)
      if toks[j] && toks[j].kind == :name && toks[j].text == "type" && toks[j + 1] && toks[j + 1].kind == :name
        k = j + 2
        k += 1 while toks[k] && !(toks[k].kind == :op && toks[k].text == "=") && toks[k].kind != :newline
        starts << k + 1 if toks[k] && toks[k].kind == :op && toks[k].text == "="
      elsif toks[j] && toks[j].kind == :name && toks[j + 1] && toks[j + 1].kind == :op && toks[j + 1].text == ":" &&
            toks[j + 2] && toks[j + 2].kind != :newline
        starts << j + 2
      end
    end
    i += 1
  end
  starts
end

# Apply the recorded rewrites innermost first, so nested `&` types compose: `&list[&str]` becomes
# `Borrowed[list[Borrowed[str]]]`.
def rewrite(source)
  toks = tokens(source)
  parser = TypeParser.new(toks, source)
  annotation_starts(toks).each { |start| parser.type_at(start) }
  edits = parser.edits.uniq.sort_by { |from, _inner_from, inner_to, _m| [inner_to - from, from] }
  bytes = source.b
  # Rewrite from the end of the file backwards; nested edits lie inside their enclosing edit, so each enclosing edit
  # is applied to text whose inner edits are already done by working inside out over a span map.
  result = bytes.dup
  applied = []
  edits.each do |from, inner_from, inner_to, mutable|
    shift = ->(pos) { applied.sum { |at, delta| at < pos ? delta : 0 } }
    f = from + shift.(from)
    inf = inner_from + shift.(inner_from)
    int = inner_to + shift.(inner_to)
    inner = result[inf...int]
    replacement = (mutable ? "BorrowedMut[" : "Borrowed[") + inner + "]"
    old_length = int - f
    result[f...int] = replacement
    applied << [from, replacement.bytesize - old_length]
  end
  [result.force_encoding("utf-8"), edits.size]
end

def incan_files(paths)
  paths.flat_map { |path| File.directory?(path) ? Dir.glob(File.join(path, "**", "*.incn")) : [path] }.sort.uniq
end

check = ARGV.delete("--check")
abort("usage: ruby borrowed_spelling.rb [--check] PATH...") if ARGV.empty?
total = 0
incan_files(ARGV).each do |path|
  source = File.read(path, encoding: "utf-8")
  rewritten, count = rewrite(source)
  next if count.zero? || rewritten == source
  total += count
  puts "#{path}\t#{count}"
  File.write(path, rewritten) unless check
end
puts "total\t#{total}"

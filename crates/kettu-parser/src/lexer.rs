//! Lexer (tokenizer) for Kettu/WIT.
//!
//! Defines the token types that the parser operates on.

/// Token types for Kettu/WIT
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Token {
    // Keywords
    Package,
    Interface,
    World,
    Func,
    Record,
    Variant,
    Enum,
    Flags,
    Resource,
    Type,
    Use,
    Import,
    Export,
    Include,
    With,
    As,
    Borrow,
    Own,
    Static,
    Constructor,
    Async,
    Await,
    From,

    // Primitives
    U8,
    U16,
    U32,
    U64,
    S8,
    S16,
    S32,
    S64,
    F32,
    F64,
    Bool,
    Char,
    String_,

    // Generic types
    List,
    Option_,
    Result_,
    Tuple,
    Future,
    Stream,

    // Kettu extensions
    Let,
    Return,

    // Literals
    Ident(String),
    Integer(String),
    StringLit(String),
    True,
    False,

    // Operators/Punctuation
    Eq,         // =
    Comma,      // ,
    Colon,      // :
    Semi,       // ;
    LParen,     // (
    RParen,     // )
    LBrace,     // {
    RBrace,     // }
    LAngle,     // <
    RAngle,     // >
    Star,       // *
    Arrow,      // ->
    Slash,      // /
    Dot,        // .
    At,         // @
    Underscore, // _
    Percent,    // %
}

impl std::fmt::Display for Token {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Token::Package => write!(f, "package"),
            Token::Interface => write!(f, "interface"),
            Token::World => write!(f, "world"),
            Token::Func => write!(f, "func"),
            Token::Record => write!(f, "record"),
            Token::Variant => write!(f, "variant"),
            Token::Enum => write!(f, "enum"),
            Token::Flags => write!(f, "flags"),
            Token::Resource => write!(f, "resource"),
            Token::Type => write!(f, "type"),
            Token::Use => write!(f, "use"),
            Token::Import => write!(f, "import"),
            Token::Export => write!(f, "export"),
            Token::Include => write!(f, "include"),
            Token::With => write!(f, "with"),
            Token::As => write!(f, "as"),
            Token::Borrow => write!(f, "borrow"),
            Token::Own => write!(f, "own"),
            Token::Static => write!(f, "static"),
            Token::Constructor => write!(f, "constructor"),
            Token::Async => write!(f, "async"),
            Token::Await => write!(f, "await"),
            Token::From => write!(f, "from"),
            Token::U8 => write!(f, "u8"),
            Token::U16 => write!(f, "u16"),
            Token::U32 => write!(f, "u32"),
            Token::U64 => write!(f, "u64"),
            Token::S8 => write!(f, "s8"),
            Token::S16 => write!(f, "s16"),
            Token::S32 => write!(f, "s32"),
            Token::S64 => write!(f, "s64"),
            Token::F32 => write!(f, "f32"),
            Token::F64 => write!(f, "f64"),
            Token::Bool => write!(f, "bool"),
            Token::Char => write!(f, "char"),
            Token::String_ => write!(f, "string"),
            Token::List => write!(f, "list"),
            Token::Option_ => write!(f, "option"),
            Token::Result_ => write!(f, "result"),
            Token::Tuple => write!(f, "tuple"),
            Token::Future => write!(f, "future"),
            Token::Stream => write!(f, "stream"),
            Token::Let => write!(f, "let"),
            Token::Return => write!(f, "return"),
            Token::Ident(s) => write!(f, "{}", s),
            Token::Integer(s) => write!(f, "{}", s),
            Token::StringLit(s) => write!(f, "\"{}\"", s),
            Token::True => write!(f, "true"),
            Token::False => write!(f, "false"),
            Token::Eq => write!(f, "="),
            Token::Comma => write!(f, ","),
            Token::Colon => write!(f, ":"),
            Token::Semi => write!(f, ";"),
            Token::LParen => write!(f, "("),
            Token::RParen => write!(f, ")"),
            Token::LBrace => write!(f, "{{"),
            Token::RBrace => write!(f, "}}"),
            Token::LAngle => write!(f, "<"),
            Token::RAngle => write!(f, ">"),
            Token::Star => write!(f, "*"),
            Token::Arrow => write!(f, "->"),
            Token::Slash => write!(f, "/"),
            Token::Dot => write!(f, "."),
            Token::At => write!(f, "@"),
            Token::Underscore => write!(f, "_"),
            Token::Percent => write!(f, "%"),
        }
    }
}

/// Convert an identifier string to a token (keyword or ident)
pub fn ident_to_token(s: &str) -> Token {
    match s {
        "package" => Token::Package,
        "interface" => Token::Interface,
        "world" => Token::World,
        "func" => Token::Func,
        "record" => Token::Record,
        "variant" => Token::Variant,
        "enum" => Token::Enum,
        "flags" => Token::Flags,
        "resource" => Token::Resource,
        "type" => Token::Type,
        "use" => Token::Use,
        "import" => Token::Import,
        "export" => Token::Export,
        "include" => Token::Include,
        "with" => Token::With,
        "as" => Token::As,
        "borrow" => Token::Borrow,
        "own" => Token::Own,
        "static" => Token::Static,
        "constructor" => Token::Constructor,
        "async" => Token::Async,
        "await" => Token::Await,
        "from" => Token::From,
        "u8" => Token::U8,
        "u16" => Token::U16,
        "u32" => Token::U32,
        "u64" => Token::U64,
        "s8" => Token::S8,
        "s16" => Token::S16,
        "s32" => Token::S32,
        "s64" => Token::S64,
        "f32" => Token::F32,
        "f64" => Token::F64,
        "bool" => Token::Bool,
        "char" => Token::Char,
        "string" => Token::String_,
        "list" => Token::List,
        "option" => Token::Option_,
        "result" => Token::Result_,
        "tuple" => Token::Tuple,
        "future" => Token::Future,
        "stream" => Token::Stream,
        "let" => Token::Let,
        "return" => Token::Return,
        "true" => Token::True,
        "false" => Token::False,
        _ => Token::Ident(s.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_keywords() {
        assert_eq!(ident_to_token("package"), Token::Package);
        assert_eq!(ident_to_token("interface"), Token::Interface);
        assert_eq!(ident_to_token("world"), Token::World);
        assert_eq!(ident_to_token("func"), Token::Func);
        assert_eq!(ident_to_token("record"), Token::Record);
        assert_eq!(ident_to_token("variant"), Token::Variant);
        assert_eq!(ident_to_token("enum"), Token::Enum);
        assert_eq!(ident_to_token("flags"), Token::Flags);
        assert_eq!(ident_to_token("resource"), Token::Resource);
        assert_eq!(ident_to_token("type"), Token::Type);
        assert_eq!(ident_to_token("use"), Token::Use);
        assert_eq!(ident_to_token("import"), Token::Import);
        assert_eq!(ident_to_token("export"), Token::Export);
        assert_eq!(ident_to_token("include"), Token::Include);
        assert_eq!(ident_to_token("with"), Token::With);
        assert_eq!(ident_to_token("as"), Token::As);
        assert_eq!(ident_to_token("borrow"), Token::Borrow);
        assert_eq!(ident_to_token("own"), Token::Own);
        assert_eq!(ident_to_token("static"), Token::Static);
        assert_eq!(ident_to_token("constructor"), Token::Constructor);
        assert_eq!(ident_to_token("async"), Token::Async);
        assert_eq!(ident_to_token("await"), Token::Await);
        assert_eq!(ident_to_token("from"), Token::From);
    }

    #[test]
    fn test_primitive_types() {
        assert_eq!(ident_to_token("u8"), Token::U8);
        assert_eq!(ident_to_token("u16"), Token::U16);
        assert_eq!(ident_to_token("u32"), Token::U32);
        assert_eq!(ident_to_token("u64"), Token::U64);
        assert_eq!(ident_to_token("s8"), Token::S8);
        assert_eq!(ident_to_token("s16"), Token::S16);
        assert_eq!(ident_to_token("s32"), Token::S32);
        assert_eq!(ident_to_token("s64"), Token::S64);
        assert_eq!(ident_to_token("f32"), Token::F32);
        assert_eq!(ident_to_token("f64"), Token::F64);
        assert_eq!(ident_to_token("bool"), Token::Bool);
        assert_eq!(ident_to_token("char"), Token::Char);
        assert_eq!(ident_to_token("string"), Token::String_);
    }

    #[test]
    fn test_generic_types() {
        assert_eq!(ident_to_token("list"), Token::List);
        assert_eq!(ident_to_token("option"), Token::Option_);
        assert_eq!(ident_to_token("result"), Token::Result_);
        assert_eq!(ident_to_token("tuple"), Token::Tuple);
        assert_eq!(ident_to_token("future"), Token::Future);
        assert_eq!(ident_to_token("stream"), Token::Stream);
    }

    #[test]
    fn test_kettu_extensions() {
        assert_eq!(ident_to_token("let"), Token::Let);
        assert_eq!(ident_to_token("return"), Token::Return);
    }

    #[test]
    fn test_literals() {
        assert_eq!(ident_to_token("true"), Token::True);
        assert_eq!(ident_to_token("false"), Token::False);
    }

    #[test]
    fn test_identifiers() {
        assert_eq!(ident_to_token("foo"), Token::Ident("foo".to_string()));
        assert_eq!(
            ident_to_token("my-type"),
            Token::Ident("my-type".to_string())
        );
        assert_eq!(ident_to_token("foo123"), Token::Ident("foo123".to_string()));
        assert_eq!(
            ident_to_token("MyInterface"),
            Token::Ident("MyInterface".to_string())
        );
    }

    #[test]
    fn test_token_display() {
        assert_eq!(format!("{}", Token::Package), "package");
        assert_eq!(format!("{}", Token::Arrow), "->");
        assert_eq!(format!("{}", Token::LBrace), "{");
        assert_eq!(format!("{}", Token::RBrace), "}");
        assert_eq!(format!("{}", Token::Ident("foo".to_string())), "foo");
        assert_eq!(format!("{}", Token::Integer("42".to_string())), "42");
        assert_eq!(
            format!("{}", Token::StringLit("hello".to_string())),
            "\"hello\""
        );
    }

    #[test]
    fn test_token_equality() {
        assert_eq!(Token::Package, Token::Package);
        assert_ne!(Token::Package, Token::Interface);
        assert_eq!(
            Token::Ident("foo".to_string()),
            Token::Ident("foo".to_string())
        );
        assert_ne!(
            Token::Ident("foo".to_string()),
            Token::Ident("bar".to_string())
        );
    }
}

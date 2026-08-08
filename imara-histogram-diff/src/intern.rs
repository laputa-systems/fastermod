use std::hash::{BuildHasher as _, Hash};

use hashbrown::DefaultHashBuilder;
use hashbrown::hash_table::{Entry, HashTable};

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
#[repr(transparent)]
pub(super) struct Token(pub u32);

pub(super) struct Interner<'a> {
    tokens: Vec<&'a str>,
    table: HashTable<Token>,
    hasher: DefaultHashBuilder,
}

impl<'a> Interner<'a> {
    pub(super) fn new(capacity: usize) -> Self {
        Self {
            tokens: Vec::with_capacity(capacity),
            table: HashTable::with_capacity(capacity),
            hasher: DefaultHashBuilder::default(),
        }
    }

    pub(super) fn intern(&mut self, token: &'a str) -> Token {
        let hash = self.hasher.hash_one(token);
        match self.table.entry(
            hash,
            |&index| self.tokens[index.0 as usize] == token,
            |&index| self.hasher.hash_one(self.tokens[index.0 as usize]),
        ) {
            Entry::Occupied(entry) => *entry.get(),
            Entry::Vacant(entry) => {
                let index = Token(self.tokens.len() as u32);
                entry.insert(index);
                self.tokens.push(token);
                index
            }
        }
    }

    pub(super) fn num_tokens(&self) -> u32 {
        self.tokens.len() as u32
    }

    pub(super) fn resolve(&self, token: Token) -> &'a str {
        self.tokens[token.0 as usize]
    }
}

pub(super) fn lines<'a>(text: &'a str, interner: &mut Interner<'a>) -> Vec<Token> {
    if text.is_empty() {
        return Vec::new();
    }

    let capacity = super::estimate_line_count(text);
    let mut tokens = Vec::with_capacity(capacity);
    let mut line_start = 0;
    while let Some(index) = memchr::memchr(b'\n', &text.as_bytes()[line_start..]) {
        let line_end = line_start + index + 1;
        tokens.push(interner.intern(&text[line_start..line_end]));
        line_start = line_end;
    }
    if line_start < text.len() {
        tokens.push(interner.intern(&text[line_start..]));
    }
    tokens
}

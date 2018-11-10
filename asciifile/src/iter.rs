use crate::{Position, Span};

#[derive(Clone, Debug)]
pub struct PositionIterator<'t> {
    /// current iterator position. Always points to the next character/position
    /// to emit
    position_to_emit: Option<Position<'t>>,
}

impl<'t> Iterator for PositionIterator<'t> {
    type Item = Position<'t>;
    fn next(&mut self) -> Option<Position<'t>> {
        match self.position_to_emit {
            None => None,
            Some(ref position) => {
                let position_to_emit = position.clone();
                self.position_to_emit = position.next();
                Some(position_to_emit)
            }
        }
    }
}

impl<'t> PositionIterator<'t> {
    pub fn new(position: Option<Position<'t>>) -> Self {
        Self {
            position_to_emit: position,
        }
    }

    /// Look at the next `n` characters without advancing the iterator.
    /// If there are less than `n` characters, return `None`.
    pub fn peek_exactly(&self, n: usize) -> Option<Span<'t>> {
        match self.peek_at_most(n) {
            None => None,
            Some(ref span) if span.as_str().len() < n => None,
            span => span,
        }
    }

    /// Look at the next `n` characters without advancing the iterator.
    ///
    /// If there are less than `n` charcters left, the returned Span will be
    /// shortened to the maximal possible length. If there are no
    /// characters left, a span cannot be built, because a span must contain
    /// at least one character.
    pub fn peek_at_most(&self, n: usize) -> Option<Span<'t>> {
        debug_assert!(n >= 1);
        match self.position_to_emit {
            None => None,
            Some(ref span_start) => {
                // unwrap is save, since we are in a branch asserting
                // that the iterator is not finished => has at least character
                // `span_start` remaining
                let span_end = self.clone().take(n).last().unwrap();
                Some(Span::new(span_start.clone(), span_end))
            }
        }
    }

    pub fn matches(&self, wanted: &str) -> bool {
        match self.peek_exactly(wanted.len()) {
            None => wanted == "",
            Some(span) => span.as_str() == wanted,
        }
    }

    pub fn peek(&self) -> Option<Position<'t>> {
        self.position_to_emit.clone()
    }

    pub fn eof_reached(&self) -> bool {
        self.position_to_emit.is_none()
    }
}

#[derive(Clone, Debug)]
pub struct ReversePositionIterator<'t> {
    /// current iterator position. Always points to the next character/position
    /// to emit
    position_to_emit: Option<Position<'t>>,
}

/// Traverse a file from front to back. In contrast to `.iter().reverse()`,
/// this efficiently walks from any given Position using the `prev()` chain
/// of the doubly linked list.
// TODO: is this stupid? How is this implemented on Vec<>?
impl<'t> Iterator for ReversePositionIterator<'t> {
    type Item = Position<'t>;
    fn next(&mut self) -> Option<Position<'t>> {
        match self.position_to_emit {
            None => None,
            Some(ref position) => {
                let position_to_emit = position.clone();
                self.position_to_emit = position.prev();
                Some(position_to_emit)
            }
        }
    }
}

impl<'t> ReversePositionIterator<'t> {
    pub fn new(position: Option<Position<'t>>) -> Self {
        Self {
            position_to_emit: position,
        }
    }
}

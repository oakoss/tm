//! The screens open: a base that is never closed, and those shown over it.

use crate::Screen;

pub(crate) struct Stack {
    base: Screen,
    above: Vec<Screen>,
}

impl Stack {
    pub(crate) const fn new(base: Screen) -> Self {
        Self {
            base,
            above: Vec::new(),
        }
    }

    /// The screen shown.
    pub(crate) fn top(&mut self) -> &mut Screen {
        self.above.last_mut().unwrap_or(&mut self.base)
    }

    pub(crate) fn push(&mut self, screen: Screen) {
        self.above.push(screen);
    }

    /// The screens, the base first.
    pub(crate) fn iter(&self) -> impl Iterator<Item = &Screen> {
        std::iter::once(&self.base).chain(&self.above)
    }

    /// Closes the screen shown, unless it is the base.
    pub(crate) fn pop(&mut self) -> bool {
        self.above.pop().is_some()
    }

    #[cfg(test)]
    pub(crate) const fn base(&self) -> &Screen {
        &self.base
    }

    #[cfg(test)]
    pub(crate) const fn depth(&self) -> usize {
        1 + self.above.len()
    }
}

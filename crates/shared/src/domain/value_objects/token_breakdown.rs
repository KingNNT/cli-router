use super::token_count::TokenCount;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TokenBreakdown {
    pub input: TokenCount,
    pub output: TokenCount,
    pub reasoning: TokenCount,
    pub cache_read: TokenCount,
    pub cache_write: TokenCount,
}

impl TokenBreakdown {
    pub fn total(&self) -> TokenCount {
        self.input + self.output + self.reasoning + self.cache_read + self.cache_write
    }
}

impl std::ops::Add for TokenBreakdown {
    type Output = Self;
    fn add(self, rhs: Self) -> Self {
        Self {
            input: self.input + rhs.input,
            output: self.output + rhs.output,
            reasoning: self.reasoning + rhs.reasoning,
            cache_read: self.cache_read + rhs.cache_read,
            cache_write: self.cache_write + rhs.cache_write,
        }
    }
}

impl std::ops::AddAssign for TokenBreakdown {
    fn add_assign(&mut self, rhs: Self) {
        self.input += rhs.input;
        self.output += rhs.output;
        self.reasoning += rhs.reasoning;
        self.cache_read += rhs.cache_read;
        self.cache_write += rhs.cache_write;
    }
}

impl std::iter::Sum for TokenBreakdown {
    fn sum<I: Iterator<Item = Self>>(iter: I) -> Self {
        iter.fold(Self::default(), |a, b| a + b)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn total_sums_all_five_categories() {
        let tb = TokenBreakdown {
            input: TokenCount::new(1),
            output: TokenCount::new(2),
            reasoning: TokenCount::new(4),
            cache_read: TokenCount::new(8),
            cache_write: TokenCount::new(16),
        };
        assert_eq!(tb.total().value(), 31);
    }

    #[test]
    fn addition_is_componentwise() {
        let a = TokenBreakdown {
            input: TokenCount::new(1),
            output: TokenCount::new(2),
            reasoning: TokenCount::new(3),
            cache_read: TokenCount::new(4),
            cache_write: TokenCount::new(5),
        };
        let b = TokenBreakdown {
            input: TokenCount::new(10),
            output: TokenCount::new(20),
            reasoning: TokenCount::new(30),
            cache_read: TokenCount::new(40),
            cache_write: TokenCount::new(50),
        };
        let sum = a + b;
        assert_eq!(sum.input.value(), 11);
        assert_eq!(sum.output.value(), 22);
        assert_eq!(sum.reasoning.value(), 33);
        assert_eq!(sum.cache_read.value(), 44);
        assert_eq!(sum.cache_write.value(), 55);
    }
}

//! Account-tree data structure.
//!
//! Transactions have flat account paths (`assets:bank:checking`).
//! Reports that need hierarchical output (`accounts --tree`,
//! `balance --tree`, `navigate`) build an `Account` tree from those
//! paths: every colon becomes a level, every leaf stores the running
//! balance per commodity.
//!
//! The tree is built on demand from a `&[Located<Transaction>]` — no
//! state leaks between commands, and every command that wants its own
//! filter view simply rebuilds the tree.

use std::collections::BTreeMap;
use std::collections::HashMap;

use crate::decimal::Decimal;
use crate::filter::PatternMatcher;
use crate::parser::located::Located;
use crate::parser::transaction::Transaction;

/// Account-tree node. Each account has a local name, a cached full
/// path, a running balance per commodity, and child accounts.
#[derive(Debug)]
pub struct Account {
    pub name: String,
    pub fullname: String,
    pub depth: usize,
    pub children: BTreeMap<String, Account>,
    pub balance: BTreeMap<String, Decimal>,
}

impl Account {
    /// The invisible root node. Its children are the top-level
    /// account names.
    pub fn root() -> Self {
        Account {
            name: String::new(),
            fullname: String::new(),
            depth: 0,
            children: BTreeMap::new(),
            balance: BTreeMap::new(),
        }
    }

    /// Find or create an account by colon-separated path.
    /// `"assets:bank:checking"` walks/creates `assets → bank → checking`.
    pub fn find_or_create(&mut self, path: &str) -> &mut Account {
        if path.is_empty() {
            return self;
        }
        let (first, rest) = match path.find(':') {
            Some(pos) => (&path[..pos], &path[pos + 1..]),
            None => (path, ""),
        };

        let parent_fullname = self.fullname.clone();
        let parent_depth = self.depth;

        let child = self.children.entry(first.to_string()).or_insert_with(|| {
            let fullname = if parent_fullname.is_empty() {
                first.to_string()
            } else {
                format!("{}:{}", parent_fullname, first)
            };
            Account {
                name: first.to_string(),
                fullname,
                depth: parent_depth + 1,
                children: BTreeMap::new(),
                balance: BTreeMap::new(),
            }
        });

        if rest.is_empty() {
            child
        } else {
            child.find_or_create(rest)
        }
    }

    /// Add an amount to this account's balance.
    pub fn add_amount(&mut self, commodity: &str, value: Decimal) {
        self.balance
            .entry(commodity.to_string())
            .and_modify(|v| *v += value)
            .or_insert(value);
    }

    /// Does this account or any descendant have a non-zero
    /// display-rounded balance? `precisions` is the per-commodity
    /// display precision table from `Journal`.
    pub fn has_balance(&self, precisions: &HashMap<String, usize>) -> bool {
        if self.balance.iter().any(|(commodity, v)| {
            let precision = precisions.get(commodity).copied().unwrap_or(2);
            !v.is_display_zero(precision)
        }) {
            return true;
        }
        self.children
            .values()
            .any(|child| child.has_balance(precisions))
    }

    /// Sum of this node's balance plus every descendant's.
    pub fn total(&self) -> BTreeMap<String, Decimal> {
        let mut total = self.balance.clone();
        for child in self.children.values() {
            for (commodity, value) in child.total() {
                total
                    .entry(commodity)
                    .and_modify(|v| *v += value)
                    .or_insert(value);
            }
        }
        total
    }

    /// Depth-first walk. `f` is invoked for every account node except
    /// the invisible root.
    pub fn walk<F>(&self, f: &mut F)
    where
        F: FnMut(&Account),
    {
        for child in self.children.values() {
            f(child);
            child.walk(f);
        }
    }

    /// Build an account tree from booked transactions. Every posting
    /// with an amount contributes to its account node's balance.
    pub fn from_transactions(transactions: &[Located<Transaction>]) -> Self {
        let mut root = Self::root();
        for lt in transactions {
            for lp in &lt.value.postings {
                let p = &lp.value;
                let Some(amount) = &p.amount else { continue };
                root.find_or_create(&p.account)
                    .add_amount(&amount.commodity, amount.value);
            }
        }
        root
    }

    /// Matches pattern on this node's fullname or on any descendant.
    /// An absent matcher accepts everything.
    pub fn matches(&self, matcher: &Option<PatternMatcher>) -> bool {
        match matcher {
            None => true,
            Some(m) => {
                m.matches(&self.fullname)
                    || self.children.values().any(|child| child.matches(matcher))
            }
        }
    }

    /// Lookup by full path. Returns `None` if any segment is missing.
    pub fn find(&self, path: &str) -> Option<&Account> {
        if path.is_empty() {
            return Some(self);
        }
        let (first, rest) = match path.find(':') {
            Some(pos) => (&path[..pos], &path[pos + 1..]),
            None => (path, ""),
        };
        let child = self.children.get(first)?;
        if rest.is_empty() {
            Some(child)
        } else {
            child.find(rest)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn find_or_create_builds_nested_path() {
        let mut root = Account::root();
        root.find_or_create("assets:bank:checking");
        assert!(root.children.contains_key("assets"));
        let assets = &root.children["assets"];
        assert_eq!(assets.fullname, "assets");
        assert_eq!(assets.depth, 1);
        let bank = &assets.children["bank"];
        assert_eq!(bank.fullname, "assets:bank");
        let checking = &bank.children["checking"];
        assert_eq!(checking.fullname, "assets:bank:checking");
        assert_eq!(checking.depth, 3);
    }

    #[test]
    fn find_or_create_reuses_common_prefix() {
        let mut root = Account::root();
        root.find_or_create("assets:bank");
        root.find_or_create("assets:cash");
        assert_eq!(root.children.len(), 1);
        assert_eq!(root.children["assets"].children.len(), 2);
    }

    #[test]
    fn add_and_total() {
        let mut root = Account::root();
        root.find_or_create("assets:checking")
            .add_amount("USD", Decimal::from(1000));
        root.find_or_create("assets:savings")
            .add_amount("USD", Decimal::from(5000));
        let assets_total = root.children["assets"].total();
        assert_eq!(assets_total["USD"], Decimal::from(6000));
    }

    #[test]
    fn find_returns_existing_node() {
        let mut root = Account::root();
        root.find_or_create("expenses:food:groceries");
        assert!(root.find("expenses:food:groceries").is_some());
        assert!(root.find("expenses:food").is_some());
        assert!(root.find("expenses").is_some());
        assert!(root.find("income").is_none());
    }

    #[test]
    fn walk_visits_every_node_except_root() {
        let mut root = Account::root();
        root.find_or_create("a:b");
        root.find_or_create("a:c");
        root.find_or_create("d");
        let mut names = Vec::new();
        root.walk(&mut |acc| names.push(acc.fullname.clone()));
        assert_eq!(names.len(), 4); // a, a:b, a:c, d
    }
}

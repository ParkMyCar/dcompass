// Copyright 2020 LEXUGE
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.
//
// You should have received a copy of the GNU General Public License
// along with this program.  If not, see <http://www.gnu.org/licenses/>.

#![deny(missing_docs)]
#![deny(unsafe_code)]
// Documentation
//! This is a simple domain matching algorithm to match domains against a set of user-defined domain rules.
//!
//! Features:
//!
//! -  Super fast (197 ns per match for a 73300+ domain rule set)
//! -  No dependencies
//!
//! # Getting Started
//!
//! ```
//! use dmatcher::Dmatcher;
//! let mut matcher = Dmatcher::new();
//! matcher.insert("apple.com", "global").unwrap();
//! assert_eq!(matcher.matches("store.apple.com").unwrap(), Some("global".into()));
//! ```

use hashbrown::HashMap;
use std::sync::Arc;
use trust_dns_proto::error::ProtoResult;

/// Type alias for Dmatcher internal usages. Exposed in case that you need it.
pub type Label = Arc<str>;

#[derive(Debug, PartialEq, Clone)]
struct LevelNode {
    dst: Option<Label>,
    next_lvs: HashMap<Label, LevelNode>,
}

impl LevelNode {
    fn new() -> Self {
        Self {
            dst: None,
            next_lvs: HashMap::new(),
        }
    }
}

#[derive(Debug, Clone)]
/// Dmatcher matcher algorithm
pub struct Dmatcher {
    root: LevelNode,
}

impl Default for Dmatcher {
    fn default() -> Self {
        Self::new()
    }
}

impl Dmatcher {
    /// Create a matcher.
    pub fn new() -> Self {
        Self {
            root: LevelNode::new(),
        }
    }

    #[cfg(test)]
    fn get_root(&self) -> &LevelNode {
        &self.root
    }

    /// Pass in a string containing `\n` and get all domains inserted.
    pub fn insert_lines(&mut self, domain: String, dst: &str) -> ProtoResult<()> {
        let lvs: Vec<&str> = domain.split('\n').collect();
        for lv in lvs {
            self.insert(lv, dst)?;
        }
        Ok(())
    }

    /// Pass in a domain and insert it into the matcher.
    pub fn insert(&mut self, domain: &str, dst: &str) -> ProtoResult<()> {
        let mut lvs: Vec<&str> = domain.split('.').collect();
        lvs.reverse();
        let mut ptr = &mut self.root;
        for lv in lvs {
            if lv == "" {
                // We should not include sub-levels like ""
                continue;
            }
            ptr = ptr
                .next_lvs
                .entry(Arc::from(lv))
                .or_insert_with(LevelNode::new);
        }
        ptr.dst = Some(Arc::from(dst));
        Ok(())
    }

    /// Match the domain against inserted domain rules. If `apple.com` is inserted, then `www.apple.com` and `stores.www.apple.com` is considered as matched while `apple.cn` is not.
    pub fn matches(&self, domain: &str) -> ProtoResult<Option<Label>> {
        let mut lvs: Vec<&str> = domain.split('.').collect();
        lvs.reverse();
        let mut ptr = &self.root;
        for lv in lvs {
            if lv == "" {
                // We should not include sub-levels like ""
                continue;
            }
            if ptr.next_lvs.is_empty() {
                break;
            }
            // If not empty...
            ptr = match ptr.next_lvs.get(lv) {
                Some(v) => v,
                None => return Ok(None),
            };
        }
        Ok(ptr.dst.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::{Dmatcher, Label, LevelNode};
    use hashbrown::HashMap;
    use trust_dns_proto::error::ProtoResult;

    #[test]
    fn matches() -> ProtoResult<()> {
        let mut matcher = Dmatcher::new();
        matcher.insert("apple.com", "global")?;
        matcher.insert("apple.cn", "domestic")?;
        assert_eq!(matcher.matches("store.apple.com")?, Some("global".into()));
        assert_eq!(matcher.matches("store.apple.com.")?, Some("global".into()));
        assert_eq!(matcher.matches("baidu.com")?, None);
        assert_eq!(
            matcher.matches("你好.store.www.apple.cn")?,
            Some("domestic".into())
        );
        Ok(())
    }

    #[test]
    fn insertion() -> ProtoResult<()> {
        let mut matcher = Dmatcher::new();
        matcher.insert("apple.com", "global")?;
        matcher.insert("apple.cn", "domestic")?;
        println!("{:?}", matcher.get_root());
        assert_eq!(
            matcher.get_root(),
            &LevelNode {
                dst: None,
                next_lvs: [
                    (
                        "cn".into(),
                        LevelNode {
                            dst: None,
                            next_lvs: [(
                                "apple".into(),
                                LevelNode {
                                    dst: Some("domestic".into()),
                                    next_lvs: []
                                        .iter()
                                        .cloned()
                                        .collect::<HashMap<Label, LevelNode>>()
                                }
                            )]
                            .iter()
                            .cloned()
                            .collect::<HashMap<Label, LevelNode>>()
                        }
                    ),
                    (
                        "com".into(),
                        LevelNode {
                            dst: None,
                            next_lvs: [(
                                "apple".into(),
                                LevelNode {
                                    dst: Some("global".into()),
                                    next_lvs: []
                                        .iter()
                                        .cloned()
                                        .collect::<HashMap<Label, LevelNode>>()
                                }
                            )]
                            .iter()
                            .cloned()
                            .collect::<HashMap<Label, LevelNode>>()
                        }
                    )
                ]
                .iter()
                .cloned()
                .collect::<HashMap<Label, LevelNode>>()
            }
        );
        Ok(())
    }
}

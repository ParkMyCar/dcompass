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

pub mod rule;

use self::rule::{actions::ActionError, matchers::MatchError, Rule};
use super::upstreams::Upstreams;
use crate::{AsyncTryInto, Label, Validatable, ValidateCell};
use async_trait::async_trait;
use bytes::{Bytes, BytesMut};
use domain::base::{name::PushError, octets::ParseError, Message, ToDname};
use log::*;
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet},
    net::IpAddr,
};
use thiserror::Error;

type Result<T> = std::result::Result<T, TableError>;

/// Errors generated by the `table` section.
#[derive(Error, Debug)]
pub enum TableError {
    /// Errors related to matchers.
    #[error(transparent)]
    MatchError(#[from] MatchError),

    /// Errors related to actions
    #[error(transparent)]
    ActionError(#[from] ActionError),

    /// Some of the table rules are unused.
    #[error("Some of the rules in table are not used: {0:?}")]
    UnusedRules(HashSet<Label>),

    /// Rules are defined recursively, which is prohibited.
    #[error("The `rule` block with tag `{0}` is being recursively called in the `table` section")]
    RuleRecursion(Label),

    /// A rule is not found.
    #[error(
        "Rule with tag `{0}` is not found in the `table` section. Note that tag `start` is required"
    )]
    UndefinedTag(Label),

    /// Failed to push the record
    #[error(transparent)]
    PushError(#[from] PushError),

    /// Failed to parse the record
    #[error(transparent)]
    ParseError(#[from] ParseError),

    /// Buf is too short
    #[error(transparent)]
    ShortBuf(#[from] domain::base::ShortBuf),

    /// Failed to parse Expr
    #[error(transparent)]
    ExprError(#[from] crate::matchers::expr::ExprError),
}

/// Query Context
pub struct QueryContext {
    /// Query sender's IP address
    pub ip: IpAddr,
}

pub struct State {
    qctx: Option<QueryContext>,
    resp: Message<Bytes>,
    query: Message<Bytes>,
}

// It is strongly discouraged and meaningless to have such default other than for convenience in test
#[cfg(test)]
impl Default for State {
    fn default() -> Self {
        Self {
            resp: Message::from_octets(Bytes::from_static(&[0; 1024])).unwrap(),
            query: Message::from_octets(Bytes::from_static(&[0; 1024])).unwrap(),
            qctx: None,
        }
    }
}

// Traverse and validate the routing table.
fn traverse(
    // A bucket to count the time each tag being used.
    bucket: &mut HashMap<&Label, (ValidateCell, &dyn Rule)>,
    // Tag of the rule that we are currently on.
    tag: &Label,
) -> Result<()> {
    // Hacky workaround on the borrow checker.
    let (val, dsts) = if let Some((c, r)) = bucket.get_mut(tag) {
        (c.val(), r.dsts())
    } else {
        return Err(TableError::UndefinedTag(tag.clone()));
    };
    if val >= &1 {
        Err(TableError::RuleRecursion(tag.clone()))
    } else {
        bucket.get_mut(tag).unwrap().0.add(1);
        for dst in dsts {
            if dst != "end".into() {
                traverse(bucket, &dst)?;
            }
        }
        bucket.get_mut(tag).unwrap().0.sub(1);
        Ok(())
    }
}

/// A simple routing table.
pub struct Table {
    rules: HashMap<Label, Box<dyn Rule>>,
    // Upstreams used in this table.
    used_upstreams: Vec<Label>,
}

impl Validatable for Table {
    type Error = TableError;
    fn validate(&self, _: Option<&Vec<Label>>) -> Result<()> {
        // A bucket used to count the time each rule being used.
        let mut bucket: HashMap<&Label, (ValidateCell, &dyn Rule)> = self
            .rules
            .iter()
            .map(|(k, v)| (k, (ValidateCell::default(), v.as_ref())))
            .collect();
        traverse(&mut bucket, &"start".into())?;
        let unused: HashSet<Label> = bucket
            .into_iter()
            .filter(|(_, (c, _))| !c.used())
            .map(|(k, _)| k)
            .cloned()
            .collect();
        if unused.is_empty() {
            Ok(())
        } else {
            Err(TableError::UnusedRules(unused))
        }
    }
}

impl Table {
    /// Create a routing table from a bunch of `Rule`s.
    pub fn new(table: HashMap<Label, Box<dyn Rule>>) -> Result<Self> {
        // A bucket used to count the time each rule being used.
        let mut bucket: HashMap<&Label, (ValidateCell, &dyn Rule)> = table
            .iter()
            .map(|(k, v)| (k, (ValidateCell::default(), v.as_ref())))
            .collect();
        traverse(&mut bucket, &"start".into())?;
        let used_upstreams = bucket
            .iter()
            .filter(|(_, (c, _))| c.used())
            .flat_map(|(_, (_, v))| v.used_upstreams())
            .collect();
        let unused: HashSet<Label> = bucket
            .into_iter()
            .filter(|(_, (c, _))| !c.used())
            .map(|(k, _)| k)
            .cloned()
            .collect();
        if !unused.is_empty() {
            return Err(TableError::UnusedRules(unused));
        }
        Ok(Self {
            rules: table,
            used_upstreams,
        })
    }

    // Not intended to be used by end-users
    pub(super) fn used_upstreams(&self) -> &Vec<Label> {
        &self.used_upstreams
    }

    // Not intended to be used by end-users
    pub(super) async fn route(
        &self,
        query: Message<Bytes>,
        qctx: Option<QueryContext>,
        upstreams: &Upstreams,
    ) -> Result<Message<Bytes>> {
        let name = query.first_question().unwrap().qname().to_dname()?;
        let mut s = State {
            qctx,
            // Clone is cheap, just a ref count increment
            query: query.clone(),
            resp: query,
        };

        let mut tag = "start";
        while tag != "end" {
            tag = self
                .rules
                .get(tag)
                .unwrap()
                .route(tag, &mut s, upstreams, &name)
                .await?;
        }
        info!("domain \"{}\" has finished routing", name);

        // Reset the header to make sure it is answering the query
        let mut msg = Message::from_octets(BytesMut::from(s.resp.as_slice()))?;
        {
            let header = msg.header_mut();
            header.set_id(s.query.header().id());
            header.set_qr(true);
            header.set_opcode(s.query.header().opcode());
            header.set_rd(s.query.header().rd());
            header.set_rcode(s.resp.header().rcode());
        }
        Ok(Message::from_octets(msg.into_octets().freeze())?)
    }
}

/// A builder for the routing table.
#[derive(Serialize, Deserialize, Clone)]
pub struct TableBuilder<R: AsyncTryInto<Box<dyn Rule>, Error = TableError>>(HashMap<Label, R>);

impl<R: AsyncTryInto<Box<dyn Rule>, Error = TableError>> TableBuilder<R> {
    /// Create a `TableBuilder` from a set of rules
    pub fn from_map(table: HashMap<impl Into<Label>, R>) -> Self {
        Self(table.into_iter().map(|(k, v)| (k.into(), v)).collect())
    }

    /// Create a builder with an empty set of rules
    pub fn new() -> Self {
        Self(HashMap::new())
    }

    /// Add new rule
    pub fn add_rule(mut self, tag: impl Into<Label>, rule: R) -> Self {
        self.0.insert(tag.into(), rule);
        self
    }
}

#[async_trait]
impl<R: AsyncTryInto<Box<dyn Rule>, Error = TableError>> AsyncTryInto<Table> for TableBuilder<R> {
    type Error = TableError;

    /// Build the rounting table from a `TableBuilder`
    async fn try_into(self) -> Result<Table> {
        let mut rules = HashMap::new();
        for (tag, r) in self.0 {
            rules.insert(tag, r.try_into().await?);
        }
        Table::new(rules)
    }
}

#[cfg(test)]
mod tests {
    use super::{rule::actions::CacheMode, TableError};
    use crate::{builders::*, AsyncTryInto};

    #[tokio::test]
    async fn is_not_recursion() {
        TableBuilder::new()
            .add_rule(
                "start",
                RuleBuilders::IfBlock(IfBlockBuilder::<BuiltinMatcherBuilders, _>::new(
                    "true",
                    BranchBuilder::<BuiltinActionBuilders>::new("foo"),
                    BranchBuilder::<BuiltinActionBuilders>::new("foo"),
                )),
            )
            .add_rule(
                "foo",
                RuleBuilders::IfBlock(IfBlockBuilder::<BuiltinMatcherBuilders, _>::new(
                    "true",
                    BranchBuilder::<BuiltinActionBuilders>::default(),
                    BranchBuilder::<BuiltinActionBuilders>::default(),
                )),
            )
            .try_into()
            .await
            .ok()
            .unwrap();
    }

    #[tokio::test]
    async fn fail_table_recursion() {
        match TableBuilder::new()
            .add_rule(
                "start",
                RuleBuilders::<BuiltinMatcherBuilders, _>::SeqBlock(BranchBuilder::<
                    BuiltinActionBuilders,
                >::new("start")),
            )
            .try_into()
            .await
            .err()
            .unwrap()
        {
            TableError::RuleRecursion(_) => {}
            e => panic!("Not the right error type: {}", e),
        }
    }

    #[tokio::test]
    async fn fail_unused_rules() {
        // Both `mock` and `unused` should be unused here because the `mock` tag in query action refers to upstreams but not rules
        match TableBuilder::new()
            .add_rule(
                "start",
                RuleBuilders::<BuiltinMatcherBuilders, _>::SeqBlock(
                    BranchBuilder::new("end").add_action(BuiltinActionBuilders::Query(
                        QueryBuilder::new("mock", CacheMode::default()),
                    )),
                ),
            )
            .add_rule(
                "mock",
                RuleBuilders::<BuiltinMatcherBuilders, _>::SeqBlock(BranchBuilder::default()),
            )
            .add_rule(
                "unused",
                RuleBuilders::<BuiltinMatcherBuilders, _>::SeqBlock(BranchBuilder::default()),
            )
            .try_into()
            .await
            .err()
            .unwrap()
        {
            TableError::UnusedRules(v) => {
                // This is now order dependent because of Vec, we compare it like this
                assert_eq!(
                    v,
                    vec!["unused".into(), "mock".into()].into_iter().collect()
                )
            }
            e => panic!("Not the right error type: {}", e),
        }
    }

    #[tokio::test]
    async fn success_domain_table() {
        TableBuilder::new()
            .add_rule(
                "start",
                RuleBuilders::IfBlock(IfBlockBuilder::<BuiltinMatcherBuilders, _>::new(
                    r#"domain([file("../data/china.txt.gz")])"#,
                    BranchBuilder::new("end").add_action(BuiltinActionBuilders::Query(
                        QueryBuilder::new("mock", CacheMode::default()),
                    )),
                    BranchBuilder::new("end").add_action(BuiltinActionBuilders::Query(
                        QueryBuilder::new("another_mock", CacheMode::default()),
                    )),
                )),
            )
            .try_into()
            .await
            .ok()
            .unwrap();
    }
}

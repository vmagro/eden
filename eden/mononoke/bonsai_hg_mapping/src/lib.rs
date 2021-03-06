/*
 * Copyright (c) Facebook, Inc. and its affiliates.
 *
 * This software may be used and distributed according to the terms of the
 * GNU General Public License version 2.
 */

#![deny(warnings)]

use std::collections::HashSet;
use std::sync::Arc;

use sql::Connection;
use sql_construct::{SqlConstruct, SqlConstructFromMetadataDatabaseConfig};
use sql_ext::SqlConnections;

use abomonation_derive::Abomonation;
use anyhow::{Error, Result};
use cloned::cloned;
use context::{CoreContext, PerfCounterType};
use futures_ext::{BoxFuture, FutureExt};
use futures_old::{future, Future, IntoFuture};
use mercurial_types::{
    HgChangesetId, HgChangesetIdPrefix, HgChangesetIdsResolvedFromPrefix, HgNodeHash,
};
use mononoke_types::{ChangesetId, RepositoryId};
use sql::queries;
use stats::prelude::*;

mod caching;
mod errors;

pub use crate::caching::CachingBonsaiHgMapping;
pub use crate::errors::ErrorKind;

define_stats! {
    prefix = "mononoke.bonsai_hg_mapping";
    gets: timeseries(Rate, Sum),
    gets_master: timeseries(Rate, Sum),
    adds: timeseries(Rate, Sum),
    get_many_hg_by_prefix: timeseries(Rate, Sum),
}

#[derive(Abomonation, Clone, Debug, Eq, Hash, PartialEq)]
pub struct BonsaiHgMappingEntry {
    pub repo_id: RepositoryId,
    pub hg_cs_id: HgChangesetId,
    pub bcs_id: ChangesetId,
}

impl BonsaiHgMappingEntry {
    fn from_thrift(entry: bonsai_hg_mapping_entry_thrift::BonsaiHgMappingEntry) -> Result<Self> {
        Ok(Self {
            repo_id: RepositoryId::new(entry.repo_id.0),
            hg_cs_id: HgChangesetId::new(HgNodeHash::from_thrift(entry.hg_cs_id)?),
            bcs_id: ChangesetId::from_thrift(entry.bcs_id)?,
        })
    }

    fn into_thrift(self) -> bonsai_hg_mapping_entry_thrift::BonsaiHgMappingEntry {
        bonsai_hg_mapping_entry_thrift::BonsaiHgMappingEntry {
            repo_id: bonsai_hg_mapping_entry_thrift::RepoId(self.repo_id.id()),
            hg_cs_id: self.hg_cs_id.into_nodehash().into_thrift(),
            bcs_id: self.bcs_id.into_thrift(),
        }
    }

    pub fn new(repo_id: RepositoryId, hg_cs_id: HgChangesetId, bcs_id: ChangesetId) -> Self {
        BonsaiHgMappingEntry {
            repo_id,
            hg_cs_id,
            bcs_id,
        }
    }
}

#[derive(Debug, Clone, Eq, PartialEq, Hash)]
pub enum BonsaiOrHgChangesetIds {
    Bonsai(Vec<ChangesetId>),
    Hg(Vec<HgChangesetId>),
}

impl BonsaiOrHgChangesetIds {
    pub fn is_empty(&self) -> bool {
        match self {
            BonsaiOrHgChangesetIds::Bonsai(v) => v.is_empty(),
            BonsaiOrHgChangesetIds::Hg(v) => v.is_empty(),
        }
    }
}

impl From<ChangesetId> for BonsaiOrHgChangesetIds {
    fn from(cs_id: ChangesetId) -> Self {
        BonsaiOrHgChangesetIds::Bonsai(vec![cs_id])
    }
}

impl From<Vec<ChangesetId>> for BonsaiOrHgChangesetIds {
    fn from(cs_ids: Vec<ChangesetId>) -> Self {
        BonsaiOrHgChangesetIds::Bonsai(cs_ids)
    }
}

impl From<HgChangesetId> for BonsaiOrHgChangesetIds {
    fn from(cs_id: HgChangesetId) -> Self {
        BonsaiOrHgChangesetIds::Hg(vec![cs_id])
    }
}

impl From<Vec<HgChangesetId>> for BonsaiOrHgChangesetIds {
    fn from(cs_ids: Vec<HgChangesetId>) -> Self {
        BonsaiOrHgChangesetIds::Hg(cs_ids)
    }
}

pub trait BonsaiHgMapping: Send + Sync {
    fn add(&self, ctx: CoreContext, entry: BonsaiHgMappingEntry) -> BoxFuture<bool, Error>;

    fn get(
        &self,
        ctx: CoreContext,
        repo_id: RepositoryId,
        cs_id: BonsaiOrHgChangesetIds,
    ) -> BoxFuture<Vec<BonsaiHgMappingEntry>, Error>;

    fn get_hg_from_bonsai(
        &self,
        ctx: CoreContext,
        repo_id: RepositoryId,
        cs_id: ChangesetId,
    ) -> BoxFuture<Option<HgChangesetId>, Error> {
        self.get(ctx, repo_id, cs_id.into())
            .map(|result| result.into_iter().next().map(|entry| entry.hg_cs_id))
            .boxify()
    }

    fn get_bonsai_from_hg(
        &self,
        ctx: CoreContext,
        repo_id: RepositoryId,
        cs_id: HgChangesetId,
    ) -> BoxFuture<Option<ChangesetId>, Error> {
        self.get(ctx, repo_id, cs_id.into())
            .map(|result| result.into_iter().next().map(|entry| entry.bcs_id))
            .boxify()
    }

    fn get_many_hg_by_prefix(
        &self,
        ctx: CoreContext,
        repo_id: RepositoryId,
        cs_prefix: HgChangesetIdPrefix,
        limit: usize,
    ) -> BoxFuture<HgChangesetIdsResolvedFromPrefix, Error>;
}

impl BonsaiHgMapping for Arc<dyn BonsaiHgMapping> {
    fn add(&self, ctx: CoreContext, entry: BonsaiHgMappingEntry) -> BoxFuture<bool, Error> {
        (**self).add(ctx, entry)
    }

    fn get(
        &self,
        ctx: CoreContext,
        repo_id: RepositoryId,
        cs_id: BonsaiOrHgChangesetIds,
    ) -> BoxFuture<Vec<BonsaiHgMappingEntry>, Error> {
        (**self).get(ctx, repo_id, cs_id)
    }

    fn get_many_hg_by_prefix(
        &self,
        ctx: CoreContext,
        repo_id: RepositoryId,
        cs_prefix: HgChangesetIdPrefix,
        limit: usize,
    ) -> BoxFuture<HgChangesetIdsResolvedFromPrefix, Error> {
        (**self).get_many_hg_by_prefix(ctx, repo_id, cs_prefix, limit)
    }
}

#[derive(Clone)]
pub struct SqlBonsaiHgMapping {
    write_connection: Connection,
    read_connection: Connection,
    read_master_connection: Connection,
}

queries! {
    write InsertMapping(values: (
        repo_id: RepositoryId,
        hg_cs_id: HgChangesetId,
        bcs_id: ChangesetId,
    )) {
        insert_or_ignore,
        "{insert_or_ignore} INTO bonsai_hg_mapping (repo_id, hg_cs_id, bcs_id) VALUES {values}"
    }

    read SelectMappingByBonsai(
        repo_id: RepositoryId,
        >list bcs_id: ChangesetId
    ) -> (HgChangesetId, ChangesetId) {
        "SELECT hg_cs_id, bcs_id
         FROM bonsai_hg_mapping
         WHERE repo_id = {repo_id}
           AND bcs_id IN {bcs_id}"
    }

    read SelectMappingByHg(
        repo_id: RepositoryId,
        >list hg_cs_id: HgChangesetId
    ) -> (HgChangesetId, ChangesetId) {
        "SELECT hg_cs_id, bcs_id
         FROM bonsai_hg_mapping
         WHERE repo_id = {repo_id}
           AND hg_cs_id IN {hg_cs_id}"
    }

    read SelectHgChangesetsByRange(repo_id: RepositoryId, hg_cs_min: &[u8], hg_cs_max: &[u8], limit: usize) -> (HgChangesetId) {
        "SELECT hg_cs_id
         FROM bonsai_hg_mapping
         WHERE repo_id = {repo_id}
           AND hg_cs_id >= {hg_cs_min} AND hg_cs_id <= {hg_cs_max}
           LIMIT {limit}
        "
    }
}

impl SqlConstruct for SqlBonsaiHgMapping {
    const LABEL: &'static str = "bonsai_hg_mapping";

    const CREATION_QUERY: &'static str = include_str!("../schemas/sqlite-bonsai-hg-mapping.sql");

    fn from_sql_connections(connections: SqlConnections) -> Self {
        Self {
            write_connection: connections.write_connection,
            read_connection: connections.read_connection,
            read_master_connection: connections.read_master_connection,
        }
    }
}

impl SqlConstructFromMetadataDatabaseConfig for SqlBonsaiHgMapping {}

impl SqlBonsaiHgMapping {
    fn verify_consistency(
        &self,
        entry: BonsaiHgMappingEntry,
    ) -> impl Future<Item = (), Error = Error> {
        let BonsaiHgMappingEntry {
            repo_id,
            hg_cs_id,
            bcs_id,
        } = entry.clone();
        cloned!(self.read_master_connection);

        let by_hg = SelectMappingByHg::query(&read_master_connection, &repo_id, &[hg_cs_id]);
        let by_bcs = SelectMappingByBonsai::query(&read_master_connection, &repo_id, &[bcs_id]);

        by_hg
            .join(by_bcs)
            .and_then(move |(by_hg_rows, by_bcs_rows)| {
                match by_hg_rows.into_iter().chain(by_bcs_rows.into_iter()).next() {
                    Some(entry) if entry == (hg_cs_id, bcs_id) => Ok(()),
                    Some((hg_cs_id, bcs_id)) => Err(ErrorKind::ConflictingEntries(
                        BonsaiHgMappingEntry {
                            repo_id,
                            hg_cs_id,
                            bcs_id,
                        },
                        entry,
                    )
                    .into()),
                    None => Err(ErrorKind::RaceConditionWithDelete(entry).into()),
                }
            })
    }
}

impl BonsaiHgMapping for SqlBonsaiHgMapping {
    fn add(&self, ctx: CoreContext, entry: BonsaiHgMappingEntry) -> BoxFuture<bool, Error> {
        STATS::adds.add_value(1);
        ctx.perf_counters()
            .increment_counter(PerfCounterType::SqlWrites);

        let BonsaiHgMappingEntry {
            repo_id,
            hg_cs_id,
            bcs_id,
        } = entry.clone();

        let this = self.clone();
        InsertMapping::query(&self.write_connection, &[(&repo_id, &hg_cs_id, &bcs_id)])
            .and_then(move |result| {
                if result.affected_rows() == 1 {
                    Ok(true).into_future().left_future()
                } else {
                    this.verify_consistency(entry)
                        .map(|()| false)
                        .right_future()
                }
            })
            .boxify()
    }

    fn get(
        &self,
        ctx: CoreContext,
        repo_id: RepositoryId,
        ids: BonsaiOrHgChangesetIds,
    ) -> BoxFuture<Vec<BonsaiHgMappingEntry>, Error> {
        STATS::gets.add_value(1);
        ctx.perf_counters()
            .increment_counter(PerfCounterType::SqlReadsReplica);
        cloned!(self.read_master_connection);

        select_mapping(&self.read_connection, repo_id, &ids)
            .and_then(move |mut mappings| {
                let left_to_fetch = filter_fetched_ids(ids, &mappings[..]);

                if left_to_fetch.is_empty() {
                    Ok(mappings).into_future().left_future()
                } else {
                    STATS::gets_master.add_value(1);
                    ctx.perf_counters()
                        .increment_counter(PerfCounterType::SqlReadsMaster);
                    select_mapping(&read_master_connection, repo_id, &left_to_fetch)
                        .map(move |mut master_mappings| {
                            mappings.append(&mut master_mappings);
                            mappings
                        })
                        .right_future()
                }
            })
            .boxify()
    }

    fn get_many_hg_by_prefix(
        &self,
        ctx: CoreContext,
        repo_id: RepositoryId,
        cs_prefix: HgChangesetIdPrefix,
        limit: usize,
    ) -> BoxFuture<HgChangesetIdsResolvedFromPrefix, Error> {
        STATS::get_many_hg_by_prefix.add_value(1);
        cloned!(self.read_master_connection);
        ctx.perf_counters()
            .increment_counter(PerfCounterType::SqlReadsReplica);
        fetch_many_hg_by_prefix(&self.read_connection, repo_id, &cs_prefix, limit)
            .and_then(move |resolved_cs| {
                match resolved_cs {
                    HgChangesetIdsResolvedFromPrefix::NoMatch => {
                        ctx.perf_counters()
                            .increment_counter(PerfCounterType::SqlReadsMaster);
                        fetch_many_hg_by_prefix(&read_master_connection, repo_id, &cs_prefix, limit)
                    }
                    _ => future::ok(resolved_cs).boxify(),
                }
            })
            .boxify()
    }
}

fn fetch_many_hg_by_prefix(
    connection: &Connection,
    repo_id: RepositoryId,
    cs_prefix: &HgChangesetIdPrefix,
    limit: usize,
) -> BoxFuture<HgChangesetIdsResolvedFromPrefix, Error> {
    SelectHgChangesetsByRange::query(
        &connection,
        &repo_id,
        &cs_prefix.min_as_ref(),
        &cs_prefix.max_as_ref(),
        &(limit + 1),
    )
    .map(move |rows| {
        let mut fetched_cs: Vec<HgChangesetId> = rows.into_iter().map(|row| row.0).collect();
        match fetched_cs.len() {
            0 => HgChangesetIdsResolvedFromPrefix::NoMatch,
            1 => HgChangesetIdsResolvedFromPrefix::Single(fetched_cs[0].clone()),
            l if l <= limit => HgChangesetIdsResolvedFromPrefix::Multiple(fetched_cs),
            _ => HgChangesetIdsResolvedFromPrefix::TooMany({
                fetched_cs.pop();
                fetched_cs
            }),
        }
    })
    .boxify()
}

fn filter_fetched_ids(
    cs: BonsaiOrHgChangesetIds,
    mappings: &[BonsaiHgMappingEntry],
) -> BonsaiOrHgChangesetIds {
    match cs {
        BonsaiOrHgChangesetIds::Bonsai(cs_ids) => {
            let bcs_fetched: HashSet<_> = mappings.iter().map(|m| &m.bcs_id).collect();

            BonsaiOrHgChangesetIds::Bonsai(
                cs_ids
                    .iter()
                    .filter_map(|cs| {
                        if !bcs_fetched.contains(cs) {
                            Some(*cs)
                        } else {
                            None
                        }
                    })
                    .collect(),
            )
        }
        BonsaiOrHgChangesetIds::Hg(cs_ids) => {
            let hg_fetched: HashSet<_> = mappings.iter().map(|m| &m.hg_cs_id).collect();

            BonsaiOrHgChangesetIds::Hg(
                cs_ids
                    .iter()
                    .filter_map(|cs| {
                        if !hg_fetched.contains(cs) {
                            Some(*cs)
                        } else {
                            None
                        }
                    })
                    .collect(),
            )
        }
    }
}

fn select_mapping(
    connection: &Connection,
    repo_id: RepositoryId,
    cs_id: &BonsaiOrHgChangesetIds,
) -> BoxFuture<Vec<BonsaiHgMappingEntry>, Error> {
    cloned!(repo_id, cs_id);
    if cs_id.is_empty() {
        return future::ok(vec![]).boxify();
    }

    let rows_fut = match cs_id {
        BonsaiOrHgChangesetIds::Bonsai(bcs_ids) => {
            SelectMappingByBonsai::query(&connection, &repo_id, &bcs_ids[..]).boxify()
        }
        BonsaiOrHgChangesetIds::Hg(hg_cs_ids) => {
            SelectMappingByHg::query(&connection, &repo_id, &hg_cs_ids[..]).boxify()
        }
    };

    rows_fut
        .map(move |rows| {
            rows.into_iter()
                .map(move |(hg_cs_id, bcs_id)| BonsaiHgMappingEntry {
                    repo_id,
                    hg_cs_id,
                    bcs_id,
                })
                .collect()
        })
        .boxify()
}

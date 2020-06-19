/*
 * Copyright (c) Facebook, Inc. and its affiliates.
 *
 * This software may be used and distributed according to the terms of the
 * GNU General Public License version 2.
 */

#![deny(warnings)]

mod mem_writes_bonsai_hg_mapping;
mod mem_writes_changesets;

use anyhow::{format_err, Context, Error};
use blobrepo::{BlobRepo, DangerousOverride};
use blobstore::Blobstore;
use bonsai_hg_mapping::BonsaiHgMapping;
use bytes::Bytes;
use cacheblob::{dummy::DummyLease, LeaseOps, MemWritesBlobstore};
use changesets::{ChangesetInsert, Changesets};
use clap::{Arg, SubCommand};
use cmdlib::args;
use cmdlib::helpers::block_execute;
use context::CoreContext;
use derived_data::BonsaiDerived;
use fbinit::FacebookInit;
use filestore::{self, FilestoreConfig, StoreRequest};
use futures::{
    compat::Future01CompatExt,
    future::{self, FutureExt as _, TryFutureExt},
    stream::{self, StreamExt as _, TryStreamExt},
};
use futures_ext::{FutureExt, StreamExt};
use futures_old::Future;
use futures_old::{
    future::IntoFuture,
    stream::{self as stream_old, Stream},
};
use git2::{Oid, Repository, Sort};
use git_types::TreeHandle;
use import_tools::{
    CommitMetadata, ExtractedCommit, GitLeaf, GitPool, GitimportPreferences, GitimportTarget,
};
use linked_hash_map::LinkedHashMap;
use manifest::{bonsai_diff, BonsaiDiffFileChange, StoreLoadable};
use mercurial_types::HgManifestId;
use mononoke_types::{
    blob::BlobstoreValue, hash::RichGitSha1, typed_hash::MononokeId, BonsaiChangeset,
    BonsaiChangesetMut, ChangesetId, ContentMetadata, FileChange, MPath,
};
use slog::info;
use std::collections::{BTreeMap, HashMap};
use std::convert::TryInto;
use std::path::Path;
use std::sync::Arc;
use tokio::task;

use crate::mem_writes_bonsai_hg_mapping::MemWritesBonsaiHgMapping;
use crate::mem_writes_changesets::MemWritesChangesets;

// Refactor this a bit. Use a thread pool for git operations. Pass that wherever we use store repo.
// Transform the walk into a stream of commit + file changes.

const SUBCOMMAND_FULL_REPO: &str = "full-repo";
const SUBCOMMAND_GIT_RANGE: &str = "git-range";

const ARG_GIT_REPOSITORY_PATH: &str = "git-repository-path";
const ARG_DERIVE_TREES: &str = "derive-trees";
const ARG_DERIVE_HG: &str = "derive-hg";
const ARG_HGGIT_COMPATIBILITY: &str = "hggit-compatibility";

const ARG_GIT_FROM: &str = "git-from";
const ARG_GIT_TO: &str = "git-to";

const HGGIT_COMMIT_ID_EXTRA: &str = "convert_revision";

async fn do_upload(
    ctx: CoreContext,
    blobstore: Arc<dyn Blobstore>,
    pool: GitPool,
    oid: Oid,
) -> Result<ContentMetadata, Error> {
    let (id, bytes) = pool
        .with(move |repo| {
            let blob = repo.find_blob(oid)?;
            let bytes = Bytes::copy_from_slice(blob.content());
            let id = blob.id();
            Result::<_, Error>::Ok((id, bytes))
        })
        .await?;

    let size = bytes.len().try_into()?;
    let git_sha1 = RichGitSha1::from_bytes(Bytes::copy_from_slice(id.as_bytes()), "blob", size)?;
    let req = StoreRequest::with_git_sha1(size, git_sha1);

    let meta = filestore::store(
        blobstore,
        FilestoreConfig::default(),
        ctx,
        &req,
        stream_old::once(Ok(bytes)),
    )
    .compat()
    .await?;

    Ok(meta)
}

// TODO: Try to produce copy-info?
// TODO: Translate LFS pointers?
// TODO: Don't re-upload things we already have
fn find_file_changes<S>(
    ctx: CoreContext,
    blobstore: Arc<dyn Blobstore>,
    pool: GitPool,
    changes: S,
) -> impl Future<Item = BTreeMap<MPath, Option<FileChange>>, Error = Error>
where
    S: Stream<Item = BonsaiDiffFileChange<GitLeaf>, Error = Error>,
{
    changes
        .map(move |change| match change {
            BonsaiDiffFileChange::Changed(path, ty, GitLeaf(oid))
            | BonsaiDiffFileChange::ChangedReusedId(path, ty, GitLeaf(oid)) => {
                do_upload(ctx.clone(), blobstore.clone(), pool.clone(), oid)
                    .boxed()
                    .compat()
                    .map(move |meta| {
                        (
                            path,
                            Some(FileChange::new(meta.content_id, ty, meta.total_size, None)),
                        )
                    })
                    .left_future()
            }
            BonsaiDiffFileChange::Deleted(path) => Ok((path, None)).into_future().right_future(),
        })
        .buffer_unordered(100)
        .collect_to()
        .from_err()
}

async fn gitimport(
    ctx: &CoreContext,
    repo: &BlobRepo,
    path: &Path,
    target: GitimportTarget,
    prefs: GitimportPreferences,
) -> Result<(), Error> {
    let walk_repo = Repository::open(&path)?;
    let pool = &GitPool::new(path.to_path_buf())?;

    let mut walk = walk_repo.revwalk()?;
    walk.set_sorting(Sort::TOPOLOGICAL | Sort::REVERSE)?;
    target.populate_walk(&walk_repo, &mut walk)?;

    // TODO: Don't import everything in one go. Instead, hide things we already imported from the
    // traversal.

    let roots = &{
        let mut roots = HashMap::new();
        target.populate_roots(&ctx, &repo, &mut roots).await?;
        roots
    };

    // Kick off a stream that consumes the walk and prepared commits. Then, produce the Bonsais.

    // TODO: Make concurrency configurable below.

    let import_map: LinkedHashMap<Oid, (ChangesetId, BonsaiChangeset)> = stream::iter(walk)
        .map(|oid| async move {
            let oid = oid.with_context(|| "While walking commits")?;

            let ExtractedCommit {
                metadata,
                tree,
                parent_trees,
            } = ExtractedCommit::new(oid, pool)
                .await
                .with_context(|| format!("While extracting {}", oid))?;

            let file_changes = task::spawn(
                find_file_changes(
                    ctx.clone(),
                    repo.get_blobstore().boxed(),
                    pool.clone(),
                    bonsai_diff(ctx.clone(), pool.clone(), tree, parent_trees),
                )
                .compat(),
            )
            .await??;

            Ok((metadata, file_changes))
        })
        .buffered(20)
        .try_fold(
            LinkedHashMap::<Oid, (ChangesetId, BonsaiChangeset)>::new(),
            {
                move |mut import_map, (metadata, file_changes)| async move {
                    let CommitMetadata {
                        oid,
                        parents,
                        author,
                        message,
                        author_date,
                    } = metadata;

                    let mut extra = BTreeMap::new();
                    if prefs.hggit_compatibility {
                        extra.insert(
                            HGGIT_COMMIT_ID_EXTRA.to_string(),
                            oid.to_string().into_bytes(),
                        );
                    }

                    let parents = parents
                        .into_iter()
                        .map(|p| {
                            roots
                                .get(&p)
                                .copied()
                                .or_else(|| import_map.get(&p).map(|p| p.0))
                                .ok_or_else(|| format_err!("Commit was not imported: {}", p))
                        })
                        .collect::<Result<Vec<_>, _>>()
                        .with_context(|| format_err!("While looking for parents of {}", oid))?;

                    // TODO: Should we have further extras?
                    let bcs = BonsaiChangesetMut {
                        parents,
                        author,
                        author_date,
                        committer: None,
                        committer_date: None,
                        message,
                        extra,
                        file_changes,
                    }
                    .freeze()?;

                    // We now that the commits are in order (this is guaranteed by the Walk), so we
                    // can insert them as-is, one by one, without extra dependency / ordering checks.

                    let blob = bcs.clone().into_blob();
                    let bcs_id = *blob.id();

                    repo.blobstore()
                        .put(ctx.clone(), bcs_id.blobstore_key(), blob.into())
                        .compat()
                        .await?;

                    repo.get_changesets_object()
                        .add(
                            ctx.clone(),
                            ChangesetInsert {
                                repo_id: repo.get_repoid(),
                                cs_id: bcs_id,
                                parents: bcs.parents().collect(),
                            },
                        )
                        .compat()
                        .await?;

                    info!(ctx.logger(), "Created {:?} => {:?}", oid, bcs_id);

                    import_map.insert(oid, (bcs_id, bcs));
                    Result::<_, Error>::Ok(import_map)
                }
            },
        )
        .await?;

    info!(
        ctx.logger(),
        "{} bonsai changesets have been committed",
        import_map.len()
    );

    for reference in walk_repo.references()? {
        let reference = reference?;

        let commit = reference.peel_to_commit()?;
        let bcs_id = import_map.get(&commit.id()).map(|e| e.0);
        info!(ctx.logger(), "Ref: {:?}: {:?}", reference.name(), bcs_id);
    }

    if prefs.derive_trees {
        for (id, (bcs_id, _bcs)) in import_map.iter() {
            let commit = walk_repo.find_commit(*id)?;
            let tree_id = commit.tree()?.id();

            let derived_tree = TreeHandle::derive(ctx.clone(), repo.clone(), *bcs_id)
                .compat()
                .await?;

            let derived_tree_id = Oid::from_bytes(derived_tree.oid().as_ref())?;

            if tree_id != derived_tree_id {
                let e = format_err!(
                    "Invalid tree was derived for {:?}: {:?} (expected {:?})",
                    commit.id(),
                    derived_tree_id,
                    tree_id
                );
                Err(e)?;
            }
        }

        info!(ctx.logger(), "{} tree(s) are valid!", import_map.len());
    }

    if prefs.derive_hg {
        let mut hg_manifests: HashMap<ChangesetId, HgManifestId> = HashMap::new();

        for (id, (bcs_id, bcs)) in import_map.iter() {
            let parent_manifests = future::try_join_all(bcs.parents().map({
                let hg_manifests = &hg_manifests;
                move |p| async move {
                    let manifest = if let Some(manifest) = hg_manifests.get(&p) {
                        *manifest
                    } else {
                        repo.get_hg_from_bonsai_changeset(ctx.clone(), p)
                            .compat()
                            .await?
                            .load(ctx.clone(), repo.blobstore())
                            .compat()
                            .await?
                            .manifestid()
                    };
                    Result::<_, Error>::Ok(manifest)
                }
            }))
            .await?;

            let manifest = repo
                .get_manifest_from_bonsai(ctx.clone(), bcs.clone(), parent_manifests)
                .compat()
                .await?;

            hg_manifests.insert(*bcs_id, manifest);

            info!(ctx.logger(), "Hg: {:?}: {:?}", id, manifest);
        }
    }

    Ok(())
}

#[fbinit::main]
fn main(fb: FacebookInit) -> Result<(), Error> {
    let app = args::MononokeApp::new("Mononoke Git Importer")
        .with_advanced_args_hidden()
        .build()
        .arg(
            Arg::with_name(ARG_DERIVE_TREES)
                .long(ARG_DERIVE_TREES)
                .required(false)
                .takes_value(false),
        )
        .arg(
            Arg::with_name(ARG_DERIVE_HG)
                .long(ARG_DERIVE_HG)
                .required(false)
                .takes_value(false),
        )
        .arg(
            Arg::with_name(ARG_HGGIT_COMPATIBILITY)
                .long(ARG_HGGIT_COMPATIBILITY)
                .help("Set commit extras for hggit compatibility")
                .required(false)
                .takes_value(false),
        )
        .arg(Arg::with_name(ARG_GIT_REPOSITORY_PATH).help("Path to a git repository to import"))
        .subcommand(SubCommand::with_name(SUBCOMMAND_FULL_REPO))
        .subcommand(
            SubCommand::with_name(SUBCOMMAND_GIT_RANGE)
                .arg(
                    Arg::with_name(ARG_GIT_FROM)
                        .required(true)
                        .takes_value(true),
                )
                .arg(Arg::with_name(ARG_GIT_TO).required(true).takes_value(true)),
        );

    let mut prefs = GitimportPreferences::default();

    let matches = app.get_matches();

    // if we are readonly, then we'll set up some overrides to still be able to do meaningful
    // things below.
    if args::parse_readonly_storage(&matches).0 {
        prefs.enable_dry_run();
    }

    if matches.is_present(ARG_DERIVE_TREES) {
        prefs.enable_derive_trees();
    }

    if matches.is_present(ARG_DERIVE_HG) {
        prefs.enable_derive_hg();
    }

    if matches.is_present(ARG_HGGIT_COMPATIBILITY) {
        prefs.enable_hggit_compatibility();
    }

    let target = match matches.subcommand() {
        (SUBCOMMAND_FULL_REPO, Some(..)) => GitimportTarget::FullRepo,
        (SUBCOMMAND_GIT_RANGE, Some(range_matches)) => {
            let from = range_matches.value_of(ARG_GIT_FROM).unwrap().parse()?;
            let to = range_matches.value_of(ARG_GIT_TO).unwrap().parse()?;
            GitimportTarget::GitRange(from, to)
        }
        _ => {
            return Err(Error::msg("A valid subcommand is required"));
        }
    };

    let path = Path::new(matches.value_of(ARG_GIT_REPOSITORY_PATH).unwrap());

    args::init_cachelib(fb, &matches, None);
    let logger = args::init_logging(fb, &matches);
    let ctx = CoreContext::new_with_logger(fb, logger.clone());

    let repo = args::create_repo(fb, &logger, &matches);

    block_execute(
        async {
            let repo = repo.compat().await?;

            let repo = if prefs.dry_run {
                repo.dangerous_override(|blobstore| -> Arc<dyn Blobstore> {
                    Arc::new(MemWritesBlobstore::new(blobstore))
                })
                .dangerous_override(|changesets| -> Arc<dyn Changesets> {
                    Arc::new(MemWritesChangesets::new(changesets))
                })
                .dangerous_override(|bonsai_hg_mapping| -> Arc<dyn BonsaiHgMapping> {
                    Arc::new(MemWritesBonsaiHgMapping::new(bonsai_hg_mapping))
                })
                .dangerous_override(|_| Arc::new(DummyLease {}) as Arc<dyn LeaseOps>)
            } else {
                repo
            };

            gitimport(&ctx, &repo, &path, target, prefs).await
        },
        fb,
        "gitimport",
        &logger,
        &matches,
        cmdlib::monitoring::AliveService,
    )
}

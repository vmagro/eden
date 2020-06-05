// @generated SignedSource<<a59a0788a05da7d4e0f340180110026d>>
// DO NOT EDIT THIS FILE MANUALLY!
// This file is a mechanical copy of the version in the configerator repo. To
// modify it, edit the copy in the configerator repo instead and copy it over by
// running the following in your fbcode directory:
//
// configerator-thrift-updater scm/mononoke/repos/commitsync.thrift

/*
 * Copyright (c) Facebook, Inc. and its affiliates.
 *
 * This software may be used and distributed according to the terms of the
 * GNU General Public License version 2.
 */

include "configerator/structs/scm/mononoke/repos/repos.thrift"

namespace py configerator.mononoke.commitsync

typedef string LargeRepoName

struct RawCommitSyncConfigAllVersionsOneRepo {
    /// All versions of `RawCommitSyncConfig` ever present for a given repo
    1: list<repos.RawCommitSyncConfig> versions,
    /// Current version of `RawCommitSyncConfig` used by a given repo
    2: string current_version,
}

struct RawCommitSyncAllVersions {
    /// All versions of `RawCommitSyncConfig` for all known repos
    1: map<LargeRepoName, RawCommitSyncConfigAllVersionsOneRepo> repos,
}

/// Current versions of commit sync maps for all known repos
struct RawCommitSyncCurrentVersions {
    1: map<LargeRepoName, repos.RawCommitSyncConfig> (rust.type = "HashMap") repos,
}
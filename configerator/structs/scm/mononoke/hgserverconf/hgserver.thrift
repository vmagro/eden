// @generated SignedSource<<29e4304d24d6f2f8b5dcd6234c8a516c>>
// DO NOT EDIT THIS FILE MANUALLY!
// This file is a mechanical copy of the version in the configerator repo. To
// modify it, edit the copy in the configerator repo instead and copy it over by
// running the following in your fbcode directory:
//
// configerator-thrift-updater scm/mononoke/hgserverconf/hgserver.thrift

namespace py3 configerator

struct DBConfig {
    1: i32 order_key,
    2: string db_tier,
    3: string db_engine,
}

struct ServerConfig {
    1: DBConfig sql_conf_default,
    2: map<string, DBConfig> sql_confs,
}
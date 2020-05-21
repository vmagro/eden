/*
 * Copyright (c) Facebook, Inc. and its affiliates.
 *
 * This software may be used and distributed according to the terms of the
 * GNU General Public License version 2.
 */

#![allow(non_camel_case_types)]

use std::fs;

use cpython::*;
use cpython_ext::{error::ResultPyErrExt, PyNone, PyPathBuf};

use dynamicconfig::Generator;
use pyconfigparser::config;

pub fn init_module(py: Python, package: &str) -> PyResult<PyModule> {
    let name = [package, "dynamicconfig"].join(".");
    let m = PyModule::new(py, &name)?;
    m.add(
        py,
        "applydynamicconfig",
        py_fn!(py, applydynamicconfig(config: config, repo_name: String)),
    )?;
    m.add(
        py,
        "generatedynamicconfig",
        py_fn!(
            py,
            generatedynamicconfig(repo_name: String, shared_path: PyPathBuf)
        ),
    )?;
    Ok(m)
}

fn applydynamicconfig(py: Python, config: config, repo_name: String) -> PyResult<PyNone> {
    let dyn_cfg = Generator::new(repo_name)
        .map_pyerr(py)?
        .execute()
        .map_pyerr(py)?;
    for section in dyn_cfg.sections() {
        for key in dyn_cfg.keys(section.clone()).iter_mut() {
            if let Some(value) = dyn_cfg.get(section.clone(), key.clone()) {
                config.set(
                    py,
                    section.to_string(),
                    key.to_string(),
                    Some(value.to_string()),
                    "hgrc.dynamic".into(),
                )?;
            }
        }
    }

    Ok(PyNone)
}

fn generatedynamicconfig(
    py: Python,
    repo_name: String,
    shared_path: PyPathBuf,
) -> PyResult<PyNone> {
    let config = Generator::new(repo_name)
        .map_pyerr(py)?
        .execute()
        .map_pyerr(py)?;
    let config_str = config.to_string();
    let config_str = format!(
        "# version={}\n# Generated by `hg debugdynamicconfig` - DO NOT MODIFY\n{}",
        ::version::VERSION,
        config_str
    );

    fs::write(shared_path.as_path().join("hgrc.dynamic"), config_str).map_pyerr(py)?;
    Ok(PyNone)
}
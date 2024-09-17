use clap::Parser;
use serde::{Deserialize, Serialize};

use crate::messages::{MSG_NO_DEPS_HELP, MSG_TESTS_EXTERNAL_NODE_HELP, MSG_TEST_PATTERN_HELP};

#[derive(Debug, Serialize, Deserialize, Parser)]
pub struct FeesArgs {
    #[clap(short, long, help = MSG_TESTS_EXTERNAL_NODE_HELP)]
    pub external_node: bool,
    #[clap(short, long, help = MSG_NO_DEPS_HELP)]
    pub no_deps: bool,
}
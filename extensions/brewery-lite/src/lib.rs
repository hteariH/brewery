use zed_extension_api as zed;

mod daemon_jar;
mod java;

struct BreweryLite;

impl zed::Extension for BreweryLite {
    fn new() -> Self {
        BreweryLite
    }
}

zed::register_extension!(BreweryLite);

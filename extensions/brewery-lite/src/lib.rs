use zed_extension_api as zed;

struct BreweryLite;

impl zed::Extension for BreweryLite {
    fn new() -> Self {
        BreweryLite
    }
}

zed::register_extension!(BreweryLite);

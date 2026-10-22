//! The icons the app registers with `Application::with_assets`: GPUI Kit's
//! default bundle, which its components draw from, and the few more the sidebar
//! needs (the list and the funnel), which that bundle leaves out.

use std::borrow::Cow;

use gpui::{AssetSource, Result, SharedString};
use gpui_kit::assets::{Assets, icon_assets};

// Lucide icons from Kit's complete catalog (`gpui_kit::assets::IconName`), on
// top of the default bundle. `Map`, the sidebar's other icon, is in it already.
icon_assets!(SidebarIcons, [List, Funnel]);

/// GPUI Kit's bundled icons, with the sidebar's list and funnel added. Register
/// it with `Application::with_assets(KitAssets)`, or the components and the
/// sidebar draw no icons; the screenshot harness does.
///
/// Anything else is answered by Kit's own [`Assets`], errors included, so a
/// path that is nowhere fails as it does there.
#[derive(Clone, Copy, Debug, Default)]
pub struct KitAssets;

impl AssetSource for KitAssets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        if let Some(icon) = SidebarIcons.load(path)? {
            return Ok(Some(icon));
        }
        Assets.load(path)
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        let mut paths = Assets.list(path)?;
        for icon in SidebarIcons.list(path)? {
            if !paths.contains(&icon) {
                paths.push(icon);
            }
        }
        Ok(paths)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_bundle_has_kits_icons_and_the_sidebars() {
        for path in [
            "icons/arrow-left.svg",
            "icons/map.svg",
            "icons/list.svg",
            "icons/funnel.svg",
        ] {
            let bytes = KitAssets.load(path).unwrap().expect(path);
            assert!(
                std::str::from_utf8(&bytes).unwrap().contains("<svg"),
                "{path}"
            );
        }
        assert!(KitAssets.load("icons/no-such-icon.svg").is_err());
    }

    #[test]
    fn the_listing_has_each_icon_once() {
        let listed = KitAssets.list("icons/").unwrap();
        for path in ["icons/map.svg", "icons/list.svg", "icons/funnel.svg"] {
            let count = listed.iter().filter(|listed| *listed == path).count();
            assert_eq!(count, 1, "{path}");
        }
    }
}

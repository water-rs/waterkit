//! Unsupported-platform shim: no application menu-bar object exists, so no
//! `MenuBar` can be built. This is an honest failure, not a fake bar.

use crate::{CommandId, MenuError, Submenu};

#[derive(Debug)]
pub struct MenuBarInner {
    _private: (),
}

impl MenuBarInner {
    pub(crate) fn new(
        _menus: impl IntoIterator<Item = Submenu>,
        _sender: &async_channel::Sender<CommandId>,
    ) -> Result<Self, MenuError> {
        Err(MenuError::Unsupported)
    }

    #[expect(
        clippy::unused_self,
        reason = "the facade calls every platform's backend through the same &self methods"
    )]
    pub(crate) fn set_enabled(&self, _id: CommandId, _enabled: bool) {
        unreachable!("no MenuBar exists on this platform")
    }

    #[expect(
        clippy::unused_self,
        reason = "the facade calls every platform's backend through the same &self methods"
    )]
    pub(crate) fn set_checked(&self, _id: CommandId, _checked: bool) {
        unreachable!("no MenuBar exists on this platform")
    }
}

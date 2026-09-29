pub mod groups;
use codewhale_command_contract::handler::{CommandContexts, CommandHandler};
use codewhale_command_contract::metadata::{CommandInfo, RegisterCommand};
use codewhale_command_contract::outcome::StructcopyCommandResult;

pub fn info() -> &'static CommandInfo {
    groups::session::structcopy::StructcopyCmd::info()
}
pub fn execute(contexts: CommandContexts<'_>, args: Option<&str>) -> StructcopyCommandResult {
    let CommandHandler::Contextual { handler, .. } =
        groups::session::structcopy::StructcopyCmd::handler()
    else {
        unreachable!("structcopy requires its declared capabilities")
    };
    handler(contexts, args)
}

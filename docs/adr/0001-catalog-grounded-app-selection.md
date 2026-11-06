# Ground natural-language app selection in the local catalog

**Status:** Accepted

Users should be able to refer to apps naturally without manually teaching DeskAct an alias for every phrase. The primary model may resolve non-exact app references by selecting a supplied discovered/configured catalog entry or requesting clarification; exact canonical names and registered workflows may use deterministic fast paths. The model cannot invent app identities or executable targets, and Rust validates the chosen entry before launch. Explicit custom app targets remain user-configured safety data; aliases are optional convenience data, and persistent agent memory is not an app-identity source.

"use client";

import SearchSelect, { type Option, type OptionSource } from "./SearchSelect";
import { useFieldControl } from "./Field";

export type { Option as ComboboxOption, OptionSource as ComboboxSource };

/**
 * Searchable single select with server-side search (SearchSelect, wired to a surrounding Field).
 * Build `source` with the helpers in lib/entitySources.ts for contacts, organizations, users...
 */
export function Combobox(props: React.ComponentProps<typeof SearchSelect>) {
  const ctl = useFieldControl({ id: props.id, "aria-describedby": props["aria-describedby"] });
  return <SearchSelect {...props} id={ctl.id} aria-describedby={ctl["aria-describedby"]} />;
}

export default Combobox;

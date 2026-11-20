"use client";

import { Monitor, Moon, Sun } from "lucide-react";
import { DropdownMenu, IconButton, MenuRadioGroup } from "@/components/ui";
import { useTheme, type ThemePref } from "@/lib/theme";

export const THEME_OPTIONS = [
  { value: "system", label: "Same as device", icon: Monitor },
  { value: "light", label: "Light", icon: Sun },
  { value: "dark", label: "Dark", icon: Moon },
];

/** A small theme switch for pages outside the ERP shell (portals, public pages). */
export function ThemeMenu() {
  const { theme, resolved, setTheme } = useTheme();
  return (
    <DropdownMenu trigger={<IconButton label="Theme" icon={resolved === "dark" ? Moon : Sun} />} className="w-52">
      <MenuRadioGroup label="Theme" value={theme} onValueChange={(v) => setTheme(v as ThemePref)} options={THEME_OPTIONS} />
    </DropdownMenu>
  );
}

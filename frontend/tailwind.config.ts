import type { Config } from "tailwindcss";

// Every color is a CSS variable (globals.css) holding "r g b" channels, so opacity modifiers
// (bg-brand/10) keep working and dark mode is a variable swap, not a second class set.
const v = (name: string) => `rgb(var(--${name}) / <alpha-value>)`;

const config: Config = {
  darkMode: ["class"],
  content: [
    "./src/pages/**/*.{js,ts,jsx,tsx,mdx}",
    "./src/components/**/*.{js,ts,jsx,tsx,mdx}",
    "./src/app/**/*.{js,ts,jsx,tsx,mdx}",
    "./src/lib/**/*.{js,ts,jsx,tsx}",
  ],
  theme: {
    extend: {
      colors: {
        // Design tokens (see docs/UI_COMPONENTS.md)
        canvas: v("canvas"),
        surface: { DEFAULT: v("surface"), 2: v("surface-2"), raised: v("surface-raised") },
        line: { DEFAULT: v("line"), soft: v("line-soft"), strong: v("line-strong") },
        fg: { DEFAULT: v("fg"), muted: v("fg-muted"), subtle: v("fg-subtle") },
        brand: { DEFAULT: v("brand"), hover: v("brand-hover"), ink: v("brand-ink"), on: v("on-brand") },
        nav: { DEFAULT: v("nav"), fg: v("nav-fg"), active: v("nav-active"), hover: v("nav-hover") },
        tone: {
          neutral: { DEFAULT: v("tone-neutral"), soft: v("tone-neutral-soft") },
          blue: { DEFAULT: v("tone-blue"), soft: v("tone-blue-soft") },
          violet: { DEFAULT: v("tone-violet"), soft: v("tone-violet-soft") },
          orange: { DEFAULT: v("tone-orange"), soft: v("tone-orange-soft") },
          red: { DEFAULT: v("tone-red"), soft: v("tone-red-soft") },
        },

        // shadcn-style names kept for existing code
        border: v("line"),
        input: v("line-strong"),
        ring: v("brand"),
        background: v("canvas"),
        foreground: v("fg"),
        primary: { DEFAULT: v("brand"), foreground: v("on-brand") },
        secondary: { DEFAULT: v("surface-2"), foreground: v("fg") },
        destructive: { DEFAULT: v("tone-red"), foreground: v("on-brand") },
        muted: { DEFAULT: v("surface-2"), foreground: v("fg-muted") },
        accent: { DEFAULT: v("surface-2"), foreground: v("fg") },
        popover: { DEFAULT: v("surface-raised"), foreground: v("fg") },
        card: { DEFAULT: v("surface"), foreground: v("fg") },
      },
      borderRadius: {
        // 8 px everywhere (D4-b); pills use rounded-full
        lg: "var(--radius)",
        md: "var(--radius)",
        sm: "calc(var(--radius) - 2px)",
      },
      boxShadow: {
        card: "var(--shadow-card)",
        "card-lg": "var(--shadow-pop)",
        pop: "var(--shadow-pop)",
        "nav-glow": "0 0 14px rgba(200, 0, 10, 0.35)",
        topbar: "0 1px 0 rgb(var(--line))",
      },
      fontFamily: {
        sans: ["var(--font-sans)", "system-ui", "sans-serif"],
        // old names, same face
        syne: ["var(--font-sans)", "system-ui", "sans-serif"],
        mono: ["var(--font-mono)", "ui-monospace", "monospace"],
      },
      fontWeight: {
        // IBM Plex ships 400/500/600 here; heavier requests use 600 instead of a faux bold
        bold: "600",
        extrabold: "600",
        black: "600",
      },
      height: { row: "38px" },
    },
  },
  plugins: [require("tailwindcss-animate")],
};

export default config;

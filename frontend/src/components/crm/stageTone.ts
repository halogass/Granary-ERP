import { statusTone, type Tone } from "@/components/ui";

// Solid marks (kanban column dots, probability bars) in the same tone as a stage's pill.
const SOLID: Record<Tone, string> = {
  neutral: "bg-tone-neutral",
  blue: "bg-tone-blue",
  violet: "bg-tone-violet",
  orange: "bg-tone-orange",
  red: "bg-tone-red",
};

/** The solid background class for a deal stage or lead status (dots, bars). */
export function stageDotClass(family: "deal" | "lead", key: string | null | undefined) {
  return SOLID[statusTone(family, key)];
}

/** The solid background class for a tone. */
export function toneDotClass(tone: Tone) {
  return SOLID[tone];
}

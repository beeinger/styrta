import type { ComponentType } from "react";
import {
  Check,
  ChevronDown,
  ChevronUp,
  Copy,
  LocateFixed,
  MapPin,
  Minus,
  Plus,
  Users,
  Volume2,
  VolumeX,
  X,
} from "lucide-react-native";

const glyphs = {
  check: Check,
  chevronDown: ChevronDown,
  chevronUp: ChevronUp,
  copy: Copy,
  locate: LocateFixed,
  pin: MapPin,
  minus: Minus,
  plus: Plus,
  users: Users,
  volume: Volume2,
  volumeOff: VolumeX,
  close: X,
} as const;

export type IconName = keyof typeof glyphs;

type GlyphProps = {
  size?: number;
  color?: string;
  strokeWidth?: number;
};

type IconProps = GlyphProps & {
  name: IconName;
};

export function Icon({ name, size = 20, color, strokeWidth = 2 }: IconProps) {
  const Glyph = glyphs[name] as ComponentType<GlyphProps>;
  return <Glyph size={size} color={color} strokeWidth={strokeWidth} />;
}

/** Same Lucide "users" drawing as `Icon name="users"`, for the map document. */
export function usersIconSvg(color: string, size: number): string {
  return `<svg xmlns="http://www.w3.org/2000/svg" width="${size}" height="${size}" viewBox="0 0 24 24" fill="none" stroke="${color}" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M16 21v-2a4 4 0 0 0-4-4H6a4 4 0 0 0-4 4v2"/><path d="M16 3.128a4 4 0 0 1 0 7.744"/><path d="M22 21v-2a4 4 0 0 0-3-3.87"/><circle cx="9" cy="7" r="4"/></svg>`;
}

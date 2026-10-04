export const colors = {
  primary: "#aece8b",
  secondary: "#ece59c",
  tertiary: "#e8cea1",
  quaternary: "#cb8f52",
  tertiaryWash: "#F6EBDA", // 40% tertiary mixed with 60% white. Solid, so text contrast is stable.
  quaternaryWash: "rgba(203, 143, 82, 0.28)",
  glass: "rgba(255, 255, 255, 0.94)",
  glassSelected: "rgba(236, 229, 156, 0.94)", // secondary at the same alpha as glass
  primaryStrong: "#3F5C34",
  ink: "#1C1C1E",
  inkMuted: "#636366",
  line: "#D1D1D6",
  lineStrong: "#3A3A3C",
  white: "#FFFFFF",
} as const;

export const space = { xs: 4, sm: 8, md: 12, lg: 16 } as const;

export const chatCornerRadius = 24;

export const glassShadow = {
  shadowColor: "#000000",
  shadowOffset: { width: 0, height: 8 },
  shadowOpacity: 0.16,
  shadowRadius: 20,
  elevation: 8,
} as const;

export const fonts = {
  regular: "Nunito_400Regular",
  semibold: "Nunito_600SemiBold",
} as const;

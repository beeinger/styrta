export const colors = {
  primary: "#aece8b",
  secondary: "#ece59c",
  tertiary: "#e8cea1",
  quaternary: "#cb8f52",
  tertiaryWash: "#F6EBDA", // 40% tertiary mixed with 60% white. Solid, so text contrast is stable.
  tertiaryWashFade: "rgba(246, 235, 218, 0)",
  quaternaryWash: "rgba(203, 143, 82, 0.22)",
  glass: "rgba(255, 255, 255, 0.96)",
  glassSelected: "rgba(236, 229, 156, 0.96)", // secondary at the same alpha as glass
  primaryStrong: "#3F5C34",
  ink: "#1C1C1E",
  inkMuted: "#636366",
  line: "#E4E1DA",
  lineStrong: "#C8C4BB",
  white: "#FFFFFF",
  canvas: "#F3F1EC",
  danger: "#9F2D2D",
} as const;

export const space = { xs: 4, sm: 8, md: 12, lg: 16, xl: 24 } as const;

export const chatCornerRadius = 28;

export const glassShadow = {
  shadowColor: "#1C1C1E",
  shadowOffset: { width: 0, height: 6 },
  shadowOpacity: 0.12,
  shadowRadius: 16,
  elevation: 6,
} as const;

export const fonts = {
  regular: "Nunito_400Regular",
  semibold: "Nunito_600SemiBold",
} as const;

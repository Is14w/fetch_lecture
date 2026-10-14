module.exports = {
  purge: ["./index.html", "./src/**/*.{vue,js,ts,jsx,tsx}"],
  darkMode: false,
  theme: {
    extend: {
      colors: {
        ink: "#19243a",
        muted: "#748098",
        canvas: "#f6f7fb",
        brand: "#315bea",
        mint: "#dff5ee",
      },
      boxShadow: {
        card: "0 16px 50px rgba(31, 48, 83, 0.08)",
        soft: "0 8px 24px rgba(31, 48, 83, 0.06)",
      },
      fontFamily: {
        sans: ["Inter", "ui-sans-serif", "system-ui", "Segoe UI", "Microsoft YaHei", "sans-serif"],
      },
    },
  },
  variants: {
    extend: {},
  },
  plugins: [],
};
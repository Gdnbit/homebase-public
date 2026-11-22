/** @type {import('tailwindcss').Config}
 *
 * 设计风格：参考 GDNbit「文艺 + 克制科技 + 高端个人站」
 * 主色板：温暖墨色 Slate（10 阶 50-950）
 * 字体栈：sans=Inter / serif=Georgia + Noto Serif（标题）/ mono=JetBrains Mono
 */
export default {
  content: ["./index.html", "./src/**/*.{ts,tsx}"],
  darkMode: ["class"],
  theme: {
    container: {
      center: true,
      padding: "1rem",
      screens: {
        "2xl": "1400px",
      },
    },
    extend: {
      colors: {
        // shadcn/ui 兼容 token（保留以便 shadcn 组件直接用）
        border: "hsl(var(--border))",
        input: "hsl(var(--input))",
        ring: "hsl(var(--ring))",
        background: "hsl(var(--background))",
        foreground: "hsl(var(--foreground))",
        secondary: {
          DEFAULT: "hsl(var(--secondary))",
          foreground: "hsl(var(--secondary-foreground))",
        },
        muted: {
          DEFAULT: "hsl(var(--muted))",
          foreground: "hsl(var(--muted-foreground))",
        },
        accent: {
          DEFAULT: "hsl(var(--accent))",
          foreground: "hsl(var(--accent-foreground))",
        },
        destructive: {
          DEFAULT: "hsl(var(--destructive))",
          foreground: "hsl(var(--destructive-foreground))",
          strong: "hsl(var(--destructive-strong))",
        },
        success: {
          DEFAULT: "hsl(var(--success))",
          foreground: "hsl(var(--success-foreground))",
          // strong：bg-success/10 软背景上的强调文字色（保证 WCAG AA 4.5:1）
          strong: "hsl(var(--success-strong))",
        },
        warning: {
          DEFAULT: "hsl(var(--warning))",
          foreground: "hsl(var(--warning-foreground))",
          strong: "hsl(var(--warning-strong))",
        },
        card: {
          DEFAULT: "hsl(var(--card))",
          foreground: "hsl(var(--card-foreground))",
        },
        popover: {
          DEFAULT: "hsl(var(--popover))",
          foreground: "hsl(var(--popover-foreground))",
        },
        // GDNbit 主色板：温暖墨色 Slate（10 阶；优先用这个）
        primary: {
          50: "#f6f7f9",
          100: "#eceef2",
          200: "#d5d9e2",
          300: "#b1b9c9",
          400: "#8793ab",
          500: "#6a7891",
          600: "#556177",  // 主要链接色 / 按钮背景
          700: "#465061",  // 按钮 hover
          800: "#3c4452",  // 链接 hover
          900: "#353c46",
          950: "#23282f",
          DEFAULT: "#556177",         // 等同 primary-600
          foreground: "#FAFAF8",      // 暖白纸感
        },
      },
      borderRadius: {
        lg: "var(--radius)",
        md: "calc(var(--radius) - 2px)",
        sm: "calc(var(--radius) - 4px)",
      },
      fontFamily: {
        sans: ["Inter", "system-ui", "-apple-system", "Segoe UI", "sans-serif"],
        // serif 给标题用，含中文字体回退（Noto Serif SC / Source Han Serif SC / 思源宋体）
        serif: [
          "Georgia",
          "Noto Serif SC",
          "Source Han Serif SC",
          "Songti SC",
          "SimSun",
          "serif",
        ],
        mono: ["JetBrains Mono", "Fira Code", "Consolas", "Monaco", "monospace"],
      },
      letterSpacing: {
        // GDNbit section-label 用 0.15em
        widest: "0.15em",
      },
      animation: {
        "fade-in": "fadeIn 0.6s ease-out",
        "slide-up": "slideUp 0.6s ease-out",
      },
      keyframes: {
        fadeIn: {
          "0%": { opacity: "0" },
          "100%": { opacity: "1" },
        },
        slideUp: {
          "0%": { transform: "translateY(12px)", opacity: "0" },
          "100%": { transform: "translateY(0)", opacity: "1" },
        },
      },
    },
  },
  plugins: [require("@tailwindcss/typography")],
};

/** @type {import('tailwindcss').Config} */
module.exports = {
  content: [
    "./index.html",
    "./src/**/*.{js,ts,jsx,tsx}",
  ],
  darkMode: 'class',
  theme: {
    fontSize: {
      '2xs': ['0.75rem', { lineHeight: '1.25rem' }],
      xs: ['0.8125rem', { lineHeight: '1.5rem' }],
      sm: ['0.875rem', { lineHeight: '1.5rem' }],
      base: ['1rem', { lineHeight: '1.75rem' }],
      lg: ['1.125rem', { lineHeight: '1.75rem' }],
      xl: ['1.25rem', { lineHeight: '1.75rem' }],
      '2xl': ['1.5rem', { lineHeight: '2rem' }],
      '3xl': ['1.875rem', { lineHeight: '2.25rem' }],
      '4xl': ['2.25rem', { lineHeight: '2.5rem' }],
      '5xl': ['3rem', { lineHeight: '1' }],
      '6xl': ['3.75rem', { lineHeight: '1' }],
      '7xl': ['4.5rem', { lineHeight: '1' }],
      '8xl': ['6rem', { lineHeight: '1' }],
      '9xl': ['8rem', { lineHeight: '1' }],
    },
    typography: require('./typography'),
    extend: {
      dropShadow: {
        glow: [
          // "0 0px 20px rgba(255,255, 255, 0.2)",
          "0 0px 15px rgba(255, 255,255, 0.25)"
        ]
      },
      fontFamily: {
        'space': ['Space Grotesk', 'sans-serif'], 
        'spaceMono': ['Space Mono', 'sans-serif'],
        'koulen': 'var(--font-koulen-regular)'
      },
      boxShadow: {
        glow: '0 0 4px rgb(0 0 0 / 0.1)',
      },
      maxWidth: {
        lg: '33rem',
        '2xl': '40rem',
        '3xl': '50rem',
        '5xl': '66rem',
      },
      backgroundImage: {
        'iroh-kv-1': "url('/img/kv/iroh-kv-1.png')",
        'iroh-kv-2': "url('/img/kv/iroh-kv-2.png')",
        'iroh-kv-3': "url('/img/kv/iroh-kv-3.png')",
        'iroh-kv-4': "url('/img/kv/iroh-kv-4.png')",
        'iroh-kv-og-1': "url('/img/kv/iroh-kv-og-1.png')",
      },
      opacity: {
        1: '0.01',
        2.5: '0.025',
        7.5: '0.075',
        15: '0.15',
      },
      colors: {
        'n0pink': {
           100: '#FFF4F3',
           200: '#FFE9E6',
           300: '#FFD2CC',
           400: '#FFBCB3',
           500: '#FFAC9C',
           600: '#E2847D',
           700: '#CC6E66',
           800: '#AF584F',
           900: '#99463D',
        },
        'irohGray': {
           50 : "#FAFAFA",
           100 : "#F8F8F8",
           200 : "#E4E4E7",
           300 : "#D4D4D8",
           400 : "#A1A1AA",
           500 : "#71717A",
           600 : "#52525B",
           700 : "#3B3B3B",
           800 : "#27272A",
           900 : "#18181B",
           1000 : "#0E0E0F",
        },
        'irohPurple': {
          // TODO - actual color gradation
          50: "#7C7CFF",
          100: "#7C7CFF",
          200: "#7C7CFF",
          300: "#7C7CFF",
          400: "#7C7CFF",
          500: "#7C7CFF",
          600: "#7C7CFF",
          700: "#7C7CFF",
          800: "#7C7CFF",
          900: "#7C7CFF",
        }
      },
      animation: {
        fadeIn: "fadeIn 500ms ease-in forwards"
      },
      keyframes: {
        fadeIn: {
          "0%": { opacity: 0, transform: 'translateY(2vh)' },
          "100%": { opacity: 1, transform: 'translateY(0)' }
        }
      }
    },
  },
  plugins: [
    require('@tailwindcss/typography'),
    require('@headlessui/tailwindcss'),
  ],
}

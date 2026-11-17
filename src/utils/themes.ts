import { Theme } from '../types/settings';

export interface ThemeProps {
  cssVariables: any;
  id: Theme;
  name: string;
  splashImage: string;
}

export const THEMES: Array<ThemeProps> = [
  {
    id: Theme.Dark,
    name: 'settings.themes.dark',
    splashImage: '/splash-dark.jpg',
    cssVariables: {
      '--app-bg-primary': 'transparent',
      '--app-bg-secondary': 'transparent',
      '--app-surface': 'rgba(255, 255, 255, 0.07)',
      '--app-card-active': 'rgba(255, 255, 255, 0.14)',
      '--app-button-text': 'rgb(10, 10, 14)',
      '--app-text-primary': 'rgb(244, 245, 248)',
      '--app-text-secondary': 'rgba(244, 245, 248, 0.68)',
      '--app-accent': 'rgb(255, 255, 255)',
      '--app-border-color': 'rgba(255, 255, 255, 0.12)',
      '--app-hover-color': 'rgb(255, 255, 255)',
      '--app-canvas-bg': 'rgb(14, 14, 17)',
      '--app-scrim': 'rgba(0, 0, 0, 0.18)',
      '--app-photo-luminance': '0.10',
    },
  },
  {
    id: Theme.Light,
    name: 'settings.themes.light',
    splashImage: '/splash-light.jpg',
    cssVariables: {
      '--app-bg-primary': 'transparent',
      '--app-bg-secondary': 'transparent',
      '--app-surface': 'rgba(255, 255, 255, 0.55)',
      '--app-card-active': 'rgba(255, 255, 255, 0.85)',
      '--app-button-text': 'rgb(255, 255, 255)',
      '--app-text-primary': 'rgb(20, 20, 24)',
      '--app-text-secondary': 'rgba(20, 20, 24, 0.66)',
      '--app-accent': 'rgb(20, 20, 24)',
      '--app-border-color': 'rgba(20, 20, 24, 0.12)',
      '--app-hover-color': 'rgb(20, 20, 24)',
      '--app-canvas-bg': 'rgb(232, 232, 236)',
      '--app-scrim': 'rgba(255, 255, 255, 0.14)',
      '--app-photo-luminance': '0.58',
    },
  },
  {
    id: Theme.Grey,
    name: 'settings.themes.grey',
    splashImage: '/splash-grey.jpg',
    cssVariables: {
      '--app-bg-primary': 'transparent',
      '--app-bg-secondary': 'transparent',
      '--app-surface': 'rgba(255, 255, 255, 0.1)',
      '--app-card-active': 'rgba(255, 255, 255, 0.18)',
      '--app-button-text': 'rgb(20, 20, 26)',
      '--app-text-primary': 'rgb(248, 248, 250)',
      '--app-text-secondary': 'rgba(248, 248, 250, 0.72)',
      '--app-accent': 'rgb(248, 248, 250)',
      '--app-border-color': 'rgba(255, 255, 255, 0.18)',
      '--app-hover-color': 'rgb(248, 248, 250)',
      '--app-canvas-bg': 'rgb(70, 70, 78)',
      '--app-scrim': 'rgba(0, 0, 0, 0.12)',
      '--app-photo-luminance': '0.2',
    },
  },
];

export const DEFAULT_THEME_ID = Theme.Dark;

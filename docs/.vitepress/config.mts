import { defineConfig } from 'vitepress'

export default defineConfig({
  title: 'Potato Launcher',
  description: 'An easy to deploy Minecraft launcher for private servers',
  outDir: 'dist',
  cleanUrls: true,
  head: [
    [
      'link',
      {
        rel: 'icon',
        type: 'image/png',
        sizes: '96x96',
        href: '/favicon-96x96.png',
      },
    ],
    ['link', { rel: 'shortcut icon', href: '/favicon.ico' }],
  ],
  themeConfig: {
    outline: 'deep',
    sidebar: [
      { text: 'About', link: '/about' },
      {
        text: 'Setting up',
        items: [
          { text: 'Hosting', link: '/setting-up/hosting' },
          { text: 'Launcher', link: '/setting-up/launcher' },
        ],
      },
      { text: 'Creating instances', link: '/creating-instances' },
      { text: 'Servers', link: '/servers' },
      { text: 'Development', link: '/development' },
    ],
    search: {
      provider: 'local',
    },
    lastUpdated: {
      formatOptions: {
        dateStyle: 'long',
        forceLocale: true,
      },
    },

    socialLinks: [
      {
        icon: 'github',
        link: 'https://github.com/Petr1Furious/potato_launcher',
      },
    ],
  },
})

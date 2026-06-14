# Potato Launcher

A Minecraft launcher that is easy to use and easy to deploy with basic DevOps skills. Perfect for frequently changing modpacks.

## Key features

- **Simple and minimal**: Does all the hard work in the background, including syncing instance files, installing Java, and self-updating
- **Performant**: Downloads instances as fast as possible using an adaptive algorithm. Also, it's written in Rust, so *blazinglyfast* 🚀🚀🚀
- **Functional**: Comes with [a tool](/creating-instances) to easily create and deploy different versions and instances. The launcher can sync arbitrary files, including mods and configs, with or without overwriting them if they exist
- **Easy deployment (backend + frontend)**: Comes with a backend API and a web UI (admin panel) to manage instances and trigger builds. Easily deployable with Docker Compose.
- **Custom auth**: Supports custom authentication servers
- **Custom instances**: Players can create custom new instances with Fabric, Forge, NeoForge, or without a modloader for all Minecraft versions
- **Vanilla format**: Fully compatible with vanilla, Forge, Fabric, and NeoForge version metadata formats. Players can also create local vanilla instances from the launcher for any Minecraft version

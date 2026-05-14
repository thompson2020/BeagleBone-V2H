# v2h_webui

## Svelte Installation

Install latest version of [Node](https://github.com/nodesource/distributions#debian-and-ubuntu-based-distributions)

```cd svelte-skeleton-ui```

Edit IP of websocket, [replace with Indra local IP.](https://github.com/rand12345/v2h_webui/blob/6eeddafdf90680bc34c25c4c6403296605eaed9f/svelte-skeleton-ui/src/routes/%2Bpage.svelte#L123C2-L123C2) 

```npm install```

Dev server (hot reload)
```npm run dev -- --host```
Testing server (faster on raspberry pi)
```npm run build```
```npm run preview -- --host```
Production server
```npm run build```
```node build```

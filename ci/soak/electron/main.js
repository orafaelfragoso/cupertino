const { app, BrowserWindow } = require("electron");

app.whenReady().then(() => {
  new BrowserWindow({ width: 900, height: 700 }).loadURL(process.argv.at(-1));
});

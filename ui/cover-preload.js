'use strict';
const { contextBridge, ipcRenderer } = require('electron');

contextBridge.exposeInMainWorld('cover', {
  onInfo: (fn) => ipcRenderer.on('info', (_e, info) => fn(info)),
});

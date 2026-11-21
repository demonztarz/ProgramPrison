'use strict';
const { contextBridge, ipcRenderer } = require('electron');

contextBridge.exposeInMainWorld('prison', {
  getState: () => ipcRenderer.invoke('state:get'),
  createBlock: (name) => ipcRenderer.invoke('block:create', name),
  updateBlock: (block) => ipcRenderer.invoke('block:update', block),
  deleteBlock: (id) => ipcRenderer.invoke('block:delete', id),
  startBlock: (id, endsAt) => ipcRenderer.invoke('block:start', { id, endsAt }),
  setLock: (lock) => ipcRenderer.invoke('block:setLock', lock),
  challenge: (id) => ipcRenderer.invoke('block:challenge', id),
  stopBlock: (id, answer) => ipcRenderer.invoke('block:stop', { id, answer }),
  updateSettings: (s) => ipcRenderer.invoke('settings:update', s),
  setupHosts: () => ipcRenderer.invoke('hosts:setup'),
  runningApps: () => ipcRenderer.invoke('apps:running'),
  onState: (fn) => ipcRenderer.on('state', (_e, s) => fn(s)),
});

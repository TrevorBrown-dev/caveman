import { mount } from 'svelte';
import './app.css';
import App from './App.svelte';
import BoardCam from './components/BoardCam.svelte';

// `?view=boardcam`: the see-through board cam window, its own page in the same build (components/BoardCam.svelte)
const view = new URLSearchParams(window.location.search).get('view');
const app = mount(view === 'boardcam' ? BoardCam : App, { target: document.getElementById('app')! });

export default app;

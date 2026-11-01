import { mount } from 'svelte';
import './app.css';
import App from './App.svelte';
import WidgetFrame from './components/widgets/WidgetFrame.svelte';

// `?view=widget&kind=<kind>`: a widget's see-through window, its own page in the same build (lib/widgets.ts is the
// registry, components/widgets/WidgetFrame.svelte the frame); `?view=boardcam` is the board cam's old page name
const view = new URLSearchParams(window.location.search).get('view');
const app = mount(view === 'widget' || view === 'boardcam' ? WidgetFrame : App, { target: document.getElementById('app')! });

export default app;

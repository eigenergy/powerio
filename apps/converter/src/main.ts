import { mount } from 'svelte';
import '@fontsource-variable/atkinson-hyperlegible-next';
import './app.css';
import App from './App.svelte';

mount(App, { target: document.getElementById('app')! });

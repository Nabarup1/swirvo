import type { APIRoute } from 'astro';
import { iconSvg } from '../lib/logo';

// The small variant of the mark (fuller tile, wider blades), as the app uses at 16–24 px.
export const GET: APIRoute = () =>
  new Response(iconSvg(true), { headers: { 'Content-Type': 'image/svg+xml' } });

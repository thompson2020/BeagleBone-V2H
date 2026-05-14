import { createServer } from 'https';
import { readFileSync } from 'fs';
import { handler } from './build/handler.js';

const key  = readFileSync('/home/unit/192.168.10.100+3-key.pem');
const cert = readFileSync('/home/unit/192.168.10.100+3.pem');

createServer({ key, cert }, handler).listen(3000, () => {
	console.log('HTTPS server running on https://192.168.10.100:3000');
});

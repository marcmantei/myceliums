import { UserService } from './services/user.js';
import { formatName } from './utils.js';

const userService = new UserService();

export function main() {
    const user = userService.getUser('123');
    const name = formatName(user.name);
    console.log(name);
}

export const handler = async (req) => {
    const data = await processRequest(req);
    return new Response(JSON.stringify(data));
};

function processRequest(req) {
    const body = parseBody(req);
    return validateInput(body);
}

function parseBody(req) {
    return req.body;
}

function validateInput(input) {
    if (!input) throw new Error('Invalid input');
    return input;
}

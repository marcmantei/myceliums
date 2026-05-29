import { UserService } from './services/user';
import { formatName } from './utils';

const userService = new UserService();

export function main() {
    const user = userService.getUser('123');
    const name = formatName(user.name);
    console.log(name);
}

export const handler = async (req: Request): Promise<Response> => {
    const data = await processRequest(req);
    return new Response(JSON.stringify(data));
};

function processRequest(req: Request) {
    const body = parseBody(req);
    return validateInput(body);
}

function parseBody(req: Request) {
    return req.body;
}

function validateInput(input: any) {
    if (!input) throw new Error('Invalid input');
    return input;
}

const { UserService } = require('./services/user.js');
const { formatName } = require('./utils.js');

const userService = new UserService();

function main() {
    const user = userService.getUser('123');
    const name = formatName(user.name);
    console.log(name);
}

const handler = async (req) => {
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

module.exports = { main, handler };

export function formatName(name) {
    return name
        .split(' ')
        .map(word => word.charAt(0).toUpperCase() + word.slice(1))
        .join(' ');
}

export const capitalize = (str) => str.toUpperCase();

export function parseUser(data) {
    return {
        name: data.name,
        email: data.email,
        age: parseInt(data.age)
    };
}

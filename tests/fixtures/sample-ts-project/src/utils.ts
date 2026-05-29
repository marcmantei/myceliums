export function formatName(name: string): string {
    return name.trim().split(' ').map(capitalize).join(' ');
}

function capitalize(word: string): string {
    return word.charAt(0).toUpperCase() + word.slice(1).toLowerCase();
}

export const debounce = (fn: Function, delay: number) => {
    let timer: any;
    return (...args: any[]) => {
        clearTimeout(timer);
        timer = setTimeout(() => fn(...args), delay);
    };
};

export type Config = {
    apiUrl: string;
    timeout: number;
    retries: number;
};

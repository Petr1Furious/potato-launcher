#!/usr/bin/env python3

# this script is run over the files in https://github.com/PrismLauncher/meta-launcher/tree/master
# to generate library-patches.json and version-matches.json
import json
import os


# the launcher will remove all libraries with these groups
# and put patched libraries for the matching version in their place
lwjgl_group_ids = ['org.lwjgl', 'org.lwjgl.lwjgl', 'net.java.jinput', 'net.java.jutils']


def generate_patches(versions: list[str]):
    major_version_groups = ['org.lwjgl', 'org.lwjgl.lwjgl']

    directory_name = {
        'org.lwjgl.lwjgl': 'org.lwjgl',
        'org.lwjgl': 'org.lwjgl3',
    }

    combined_json = {}

    combined_json['lwjgl_group_ids'] = lwjgl_group_ids
    combined_json['overrides'] = []
    for major_version_group in major_version_groups:
        directory = directory_name[major_version_group]
        for filename in os.listdir(directory):
            if filename.removesuffix('.json') in versions:
                filepath = os.path.join(directory, filename)
                with open(filepath, 'r') as file:
                    json_content = json.load(file)
                    combined_json['overrides'].append(json_content)

    with open('library-overrides.json', 'w') as outfile:
        json.dump(combined_json, outfile, indent=4)

    print("Combined JSON file created successfully.")


def generate_version_matches() -> set[str]:
    version_matches = {}
    with open('net.minecraft/index.json', 'r') as file:
        json_content = json.load(file)
        for version in json_content['versions']:
            version_value = version.get('version')
            
            requires = version.get('requires')
            if not isinstance(requires, list) or len(requires) != 1:
                raise Exception(f"Invalid \"requires\" field for {version_value}")
            requires = requires[0]
            
            version_matches[version_value] = requires['suggests']

    with open('lwjgl-version-matches.json', 'w') as outfile:
        json.dump(version_matches, outfile, indent=4)

    print("Version matches JSON file created successfully.")
    
    return set(version_matches.values())


if __name__ == '__main__':
    versions = generate_version_matches()
    generate_patches(list(versions))
